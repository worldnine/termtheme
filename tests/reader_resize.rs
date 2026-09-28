//! 大きさの変化（SIGWINCH）が `Event::Resize` で届く。SIGWINCH はプロセス全体に届くので、
//! ほかの読み手のテストと混ざらないよう別のプロセス（このファイル）で確かめる。
//! テストは 1 本にまとめて順に確かめる（1 本の `raise` がほかのテストの読み手にも届く）。

#![cfg(unix)]

mod common;

use std::os::fd::AsRawFd;
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use termtheme::input::{Input, Reader};

#[test]
fn a_window_size_change_arrives_as_resize() {
    let mut pty = common::open_raw(80, 24);
    let mut reader = Reader::from_fd(pty.slave.try_clone().unwrap()).unwrap();

    let mut size = libc::winsize {
        ws_row: 30,
        ws_col: 100,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: 読み手が読む端末の大きさを変え、SIGWINCH を自分に送る。
    unsafe {
        assert_eq!(
            libc::ioctl(pty.slave.as_raw_fd(), libc::TIOCSWINSZ, &mut size),
            0
        );
        assert_eq!(libc::raise(libc::SIGWINCH), 0);
    }
    assert!(reader.poll(common::PATIENCE).unwrap());
    assert_eq!(reader.read().unwrap(), Input::Event(Event::Resize(100, 30)));
    // 溜まった SIGWINCH は 1 つにまとまり、大きさの変化のほかは何も出ない（外から届く
    // SIGWINCH — herdr のペインの大きさが変わったときなど — があれば、同じ大きさの
    // Resize がもう 1 つ出ることはある）。
    while reader.poll(Duration::ZERO).unwrap() {
        assert_eq!(reader.read().unwrap(), Input::Event(Event::Resize(100, 30)));
    }

    // SIGWINCH と入力が同じ回に来ても、入力を取りこぼさない（crossterm は `Resize` だけを
    // 返し、入力は次の入力まで読まない — `src/input/mod.rs` の冒頭）。
    // SAFETY: SIGWINCH を自分に送るだけ。
    assert_eq!(unsafe { libc::raise(libc::SIGWINCH) }, 0);
    pty.terminal_sends(b"a");
    let until = std::time::Instant::now() + common::PATIENCE;
    let mut got = Vec::new();
    while !got.contains(&Input::Event(Event::Key(KeyEvent::new(
        KeyCode::Char('a'),
        KeyModifiers::NONE,
    )))) {
        assert!(std::time::Instant::now() < until, "a が届かない: {got:?}");
        if reader.poll(Duration::from_millis(100)).unwrap() {
            got.push(reader.read().unwrap());
        }
    }
    assert!(
        got.contains(&Input::Event(Event::Resize(100, 30))),
        "{got:?}"
    );
}
