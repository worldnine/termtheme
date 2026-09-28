//! 大きさの変化（SIGWINCH）が `Event::Resize` で届く。SIGWINCH はプロセス全体に届くので、
//! ほかの読み手のテストと混ざらないよう別のプロセス（このファイル）で確かめる。

#![cfg(unix)]

mod common;

use std::os::fd::AsRawFd;
use std::time::Duration;

use crossterm::event::Event;
use termtheme::input::{Input, Reader};

#[test]
fn a_window_size_change_arrives_as_resize() {
    let pty = common::open_raw(80, 24);
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
    assert!(reader.poll(Duration::from_secs(1)).unwrap());
    assert_eq!(reader.read().unwrap(), Input::Event(Event::Resize(100, 30)));
    // 1 回の SIGWINCH は 1 回の Resize。
    assert!(!reader.poll(Duration::from_millis(100)).unwrap());
}
