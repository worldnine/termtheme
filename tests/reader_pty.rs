//! 読み手（[`termtheme::input::Reader`]）を実物の pty で読ませる。
//! `tests/crossterm_2031.rs` で crossterm が止まった入力を、こちらは読み切る。

#![cfg(unix)]

mod common;

use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use termtheme::input::{Input, Reader};
use termtheme::scheme::ColorScheme;

fn key(c: char) -> Input {
    Input::Event(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )))
}

/// `wait` のあいだに読めた入力を全部読む。
fn read_for(reader: &mut Reader, wait: Duration) -> Vec<Input> {
    let until = Instant::now() + wait;
    let mut out = Vec::new();
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        if !reader.poll(left).unwrap() {
            break;
        }
        out.push(reader.read().unwrap());
    }
    out
}

#[test]
fn color_scheme_reports_arrive_and_the_keys_around_them_survive() {
    let mut pty = common::open_raw(80, 24);
    let mut reader = Reader::from_fd(pty.slave.try_clone().unwrap()).unwrap();
    let settle = Duration::from_millis(300);

    pty.terminal_sends(b"a");
    pty.terminal_sends(b"\x1b[?997;1n");
    pty.terminal_sends(b"j\x1b[A");
    pty.terminal_sends(b"\x1b[?997;2nk");
    assert_eq!(
        read_for(&mut reader, settle),
        vec![
            key('a'),
            Input::ColorScheme(ColorScheme::Dark),
            key('j'),
            Input::Event(Event::Key(KeyCode::Up.into())),
            Input::ColorScheme(ColorScheme::Light),
            key('k'),
        ]
    );
}

#[test]
fn poll_returns_on_time_even_in_the_middle_of_a_sequence() {
    let mut pty = common::open_raw(80, 24);
    let mut reader = Reader::from_fd(pty.slave.try_clone().unwrap()).unwrap();

    // 列の途中で入力が途切れても、poll は締め切りで戻る（crossterm は read(2) で止まる）。
    pty.terminal_sends(b"\x1b[?99");
    let start = Instant::now();
    assert!(!reader.poll(Duration::from_millis(100)).unwrap());
    let took = start.elapsed();
    assert!(
        took >= Duration::from_millis(90) && took < Duration::from_millis(400),
        "{took:?}"
    );

    // 続きが来れば 1 つの知らせ。
    pty.terminal_sends(b"7;2n");
    assert_eq!(
        read_for(&mut reader, Duration::from_millis(300)),
        vec![Input::ColorScheme(ColorScheme::Light)]
    );

    // 何も来なければ、0 秒の poll はすぐ戻る。
    let start = Instant::now();
    assert!(!reader.poll(Duration::ZERO).unwrap());
    assert!(start.elapsed() < Duration::from_millis(50));
}

#[test]
fn background_answers_arrive_as_input_instead_of_keys() {
    let mut pty = common::open_raw(80, 24);
    let mut reader = Reader::from_fd(pty.slave.try_clone().unwrap()).unwrap();

    pty.terminal_sends(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x1b\\q");
    let got = read_for(&mut reader, Duration::from_millis(300));
    assert_eq!(got, vec![Input::Background((0xfd, 0xf6, 0xe3)), key('q')]);
    assert_eq!(got[0].light(), Some(true));
    assert_eq!(got[1].light(), None);
    assert_eq!(Input::ColorScheme(ColorScheme::Dark).light(), Some(false));
}

#[test]
fn a_user_typed_alt_bracket_and_escape_are_keys() {
    let mut pty = common::open_raw(80, 24);
    let mut reader = Reader::from_fd(pty.slave.try_clone().unwrap()).unwrap();

    pty.terminal_sends(b"\x1b]");
    assert_eq!(
        read_for(&mut reader, Duration::from_millis(200)),
        vec![Input::Event(Event::Key(KeyEvent::new(
            KeyCode::Char(']'),
            KeyModifiers::ALT
        )))]
    );
    pty.terminal_sends(b"\x1b");
    assert_eq!(
        read_for(&mut reader, Duration::from_millis(200)),
        vec![Input::Event(Event::Key(KeyCode::Esc.into()))]
    );
}

#[test]
fn a_closed_terminal_is_an_error_not_a_busy_loop() {
    let pty = common::open_raw(80, 24);
    let mut reader = Reader::from_fd(pty.slave.try_clone().unwrap()).unwrap();
    drop(pty);
    let start = Instant::now();
    let result = reader.poll(Duration::from_secs(2));
    assert!(result.is_err(), "{result:?}");
    assert!(start.elapsed() < Duration::from_secs(1));
}
