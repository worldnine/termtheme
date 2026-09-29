//! 読み手（[`termtheme::input::Reader`]）を実物の pty で読ませる。
//! `tests/crossterm_2031.rs` で crossterm が止まった入力を、こちらは読み切る。
//!
//! 時間に頼らない（`tests/common` の冒頭）: 送ったバイトは入力の列に全部載ってから読み、
//! 届くはずの入力の数だけを長い締め切りで待つ。

#![cfg(unix)]

mod common;

use std::time::{Duration, Instant};

use common::PATIENCE;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use termtheme::colors::QueryColors;
use termtheme::input::{Input, Reader};
use termtheme::scheme::ColorScheme;

fn key(c: char) -> Input {
    Input::Event(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )))
}

/// 外から届く SIGWINCH（herdr のペインの大きさが変わったときなど）の `Resize` は数えない。
fn is_resize(input: &Input) -> bool {
    matches!(input, Input::Event(Event::Resize(..)))
}

/// 入力を `n` 個読む（締め切りは [`PATIENCE`]）。
fn read_n(reader: &mut Reader, n: usize) -> Vec<Input> {
    let until = Instant::now() + PATIENCE;
    let mut got = Vec::new();
    while got.len() < n {
        assert!(Instant::now() < until, "{n} 個届かない: {got:?}");
        if reader.poll(Duration::from_millis(100)).unwrap() {
            let input = reader.read().unwrap();
            if !is_resize(&input) {
                got.push(input);
            }
        }
    }
    got
}

/// `poll(timeout)` が時間切れで `false` を返すまで呼ぶ（外からの `Resize` は読み飛ばす）。
fn poll_until_timeout(reader: &mut Reader, timeout: Duration) {
    while reader.poll(timeout).unwrap() {
        let input = reader.read().unwrap();
        assert!(is_resize(&input), "余計な入力: {input:?}");
    }
}

/// 読める入力がもう無い。
fn assert_no_more(reader: &mut Reader) {
    poll_until_timeout(reader, Duration::ZERO);
}

#[test]
fn color_scheme_reports_arrive_and_the_keys_around_them_survive() {
    let mut pty = common::open_raw(80, 24);
    let mut reader = Reader::from_fd(pty.slave.try_clone().unwrap()).unwrap();

    pty.terminal_sends(b"a");
    pty.terminal_sends(b"\x1b[?997;1n");
    pty.terminal_sends(b"j\x1b[A");
    pty.terminal_sends(b"\x1b[?997;2nk");
    assert_eq!(
        read_n(&mut reader, 6),
        vec![
            key('a'),
            Input::ColorScheme(ColorScheme::Dark),
            key('j'),
            Input::Event(Event::Key(KeyCode::Up.into())),
            Input::ColorScheme(ColorScheme::Light),
            key('k'),
        ]
    );
    assert_no_more(&mut reader);
}

#[test]
fn poll_returns_on_time_even_in_the_middle_of_a_sequence() {
    let mut pty = common::open_raw(80, 24);
    let mut reader = Reader::from_fd(pty.slave.try_clone().unwrap()).unwrap();

    // 列の途中で入力が途切れても、poll は締め切りで戻る（crossterm は read(2) で止まる）。
    // 戻るまでに締め切りの分は待つ。上の端は、止まっていないことが分かれば足りる。
    pty.terminal_sends(b"\x1b[?99");
    let start = Instant::now();
    poll_until_timeout(&mut reader, Duration::from_millis(100));
    let took = start.elapsed();
    assert!(
        took >= Duration::from_millis(100) && took < PATIENCE,
        "{took:?}"
    );

    // 続きが来れば 1 つの知らせ。
    pty.terminal_sends(b"7;2n");
    assert_eq!(
        read_n(&mut reader, 1),
        vec![Input::ColorScheme(ColorScheme::Light)]
    );

    // 何も来なければ、0 秒の poll は待たずに戻る。
    let start = Instant::now();
    assert_no_more(&mut reader);
    assert!(start.elapsed() < PATIENCE);
}

#[test]
fn background_answers_arrive_as_input_instead_of_keys() {
    let mut pty = common::open_raw(80, 24);
    let mut reader = Reader::from_fd(pty.slave.try_clone().unwrap()).unwrap();

    pty.terminal_sends(b"\x1b]11;rgb:fdfd/f6f6/e3e3\x1b\\q");
    let got = read_n(&mut reader, 2);
    assert_eq!(got, vec![Input::Background((0xfd, 0xf6, 0xe3)), key('q')]);
    assert_no_more(&mut reader);
    assert_eq!(got[0].light(), Some(true));
    assert_eq!(got[1].light(), None);
    assert_eq!(Input::ColorScheme(ColorScheme::Dark).light(), Some(false));
}

#[test]
fn foreground_and_palette_answers_arrive_as_input_instead_of_keys() {
    let mut pty = common::open_raw(80, 24);
    let mut reader = Reader::from_fd(pty.slave.try_clone().unwrap()).unwrap();

    pty.terminal_sends(b"\x1b]10;rgb:e0e0/e2e2/eaea\x1b\\\x1b]4;1;rgb:cdcd/0000/0000\x07q");
    let got = read_n(&mut reader, 3);
    assert_eq!(
        got,
        vec![
            Input::Foreground((0xe0, 0xe2, 0xea)),
            Input::Palette(1, (0xcd, 0, 0)),
            key('q'),
        ]
    );
    assert_no_more(&mut reader);
    // 明るさは背景色と配色の知らせだけで決める。
    assert_eq!(got[0].light(), None);
    assert_eq!(got[1].light(), None);
}

#[test]
fn waiting_for_colors_gathers_the_answers_and_keeps_the_keys_typed_meanwhile() {
    let mut pty = common::open_raw(80, 24);
    let mut reader = Reader::from_fd(pty.slave.try_clone().unwrap()).unwrap();

    // 待つ前から読み手に溜まっていた入力: キーと、前の問い合わせに遅れて届いた文字色の答え。
    // この問い合わせの答えとしては読まない。
    pty.terminal_sends(b"a\x1b]10;rgb:00/00/00\x1b\\");
    assert!(reader.poll(Duration::ZERO).unwrap());

    // 問い合わせの答えと、そのあいだに打たれたキー・届いた知らせ・頼んでいない色の答え。
    let query = QueryColors {
        foreground: true,
        background: true,
        palette: vec![0, 1],
    };
    pty.terminal_sends(
        b"j\x1b]10;rgb:e0/e2/ea\x1b\\\x1b]11;rgb:14/16/1b\x07k\x1b[?997;2n\
          \x1b]4;0;rgb:00/00/00\x1b\\\x1b]4;5;rgb:55/55/55\x1b\\\x1b]4;1;rgb:ff/00/00\x1b\\l",
    );
    let start = Instant::now();
    let colors = reader.wait_for_colors(&query, PATIENCE).unwrap();
    assert!(start.elapsed() < PATIENCE, "揃ったら締め切りを待たずに戻る");
    assert!(query.is_answered(&colors));
    assert_eq!(
        colors.foreground,
        Some((0xe0, 0xe2, 0xea)),
        "待つ前の答えではない"
    );
    assert_eq!(colors.background, Some((0x14, 0x16, 0x1b)));
    assert_eq!(colors.palette[0], Some((0, 0, 0)));
    assert_eq!(colors.palette[1], Some((0xff, 0, 0)));
    assert_eq!(colors.palette[5], None, "頼んでいない");

    // 答えでない入力は、待つ前からの分の後ろに、届いた順で残っている。
    assert_eq!(
        read_n(&mut reader, 7),
        vec![
            key('a'),
            Input::Foreground((0, 0, 0)),
            key('j'),
            key('k'),
            Input::ColorScheme(ColorScheme::Light),
            Input::Palette(5, (0x55, 0x55, 0x55)),
            key('l'),
        ]
    );
    assert_no_more(&mut reader);
}

#[test]
fn waiting_for_colors_gives_up_on_time_and_late_answers_arrive_as_input() {
    let mut pty = common::open_raw(80, 24);
    let mut reader = Reader::from_fd(pty.slave.try_clone().unwrap()).unwrap();

    // 背景色だけ答える端末。
    let query = QueryColors::ansi();
    pty.terminal_sends(b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\");
    let start = Instant::now();
    let colors = reader
        .wait_for_colors(&query, Duration::from_millis(100))
        .unwrap();
    let took = start.elapsed();
    assert!(
        took >= Duration::from_millis(100) && took < PATIENCE,
        "{took:?}"
    );
    assert!(!query.is_answered(&colors));
    assert_eq!(colors.background, Some((255, 255, 255)), "揃った分は返す");
    assert_eq!(colors.foreground, None);
    assert_eq!(colors.ansi(), None);
    assert_no_more(&mut reader);

    // 締め切りに遅れた答えは、捨てずに入力として届く。
    pty.terminal_sends(b"\x1b]10;rgb:00/00/00\x1b\\");
    assert_eq!(read_n(&mut reader, 1), vec![Input::Foreground((0, 0, 0))]);
    assert_no_more(&mut reader);
}

#[test]
fn a_user_typed_alt_bracket_and_escape_are_keys() {
    let mut pty = common::open_raw(80, 24);
    let mut reader = Reader::from_fd(pty.slave.try_clone().unwrap()).unwrap();

    pty.terminal_sends(b"\x1b]");
    assert_eq!(
        read_n(&mut reader, 1),
        vec![Input::Event(Event::Key(KeyEvent::new(
            KeyCode::Char(']'),
            KeyModifiers::ALT
        )))]
    );
    assert_no_more(&mut reader);
    pty.terminal_sends(b"\x1b");
    assert_eq!(
        read_n(&mut reader, 1),
        vec![Input::Event(Event::Key(KeyCode::Esc.into()))]
    );
    assert_no_more(&mut reader);
}

#[test]
fn a_closed_terminal_is_an_error_not_a_busy_loop() {
    let pty = common::open_raw(80, 24);
    let mut reader = Reader::from_fd(pty.slave.try_clone().unwrap()).unwrap();
    drop(pty);
    // 締め切りまで待って `false` ではなく、すぐに誤りを返す。
    let timeout = Duration::from_secs(60);
    let start = Instant::now();
    let result = loop {
        match reader.poll(timeout) {
            Ok(true) => assert!(is_resize(&reader.read().unwrap())),
            other => break other,
        }
    };
    assert!(result.is_err(), "{result:?}");
    assert!(start.elapsed() < timeout);
}
