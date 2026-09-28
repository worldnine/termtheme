//! pty を使うテストの補助。

#![allow(dead_code)]

use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// 開いた pty。`master` が端末の側、`slave` がアプリの側。
pub struct Pty {
    pub master: std::fs::File,
    pub slave: OwnedFd,
}

/// pty を開き、アプリの側を raw モードにする（行単位の入力のままでは改行まで読めない）。
pub fn open_raw(columns: u16, rows: u16) -> Pty {
    let (mut master, mut slave) = (-1, -1);
    let mut size = libc::winsize {
        ws_row: rows,
        ws_col: columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: 出力先の fd 2 つと winsize を渡す（macOS は `*mut`、Linux は `*const` を取る）。
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    assert_eq!(rc, 0, "openpty: {}", std::io::Error::last_os_error());
    // SAFETY: openpty が開いた fd の持ち主になる。
    let (master, slave) = unsafe {
        (
            std::fs::File::from_raw_fd(master),
            OwnedFd::from_raw_fd(slave),
        )
    };
    // SAFETY: termios を読み、cfmakeraw で raw にして書き戻す。
    unsafe {
        let mut ios: libc::termios = std::mem::zeroed();
        assert_eq!(libc::tcgetattr(slave.as_raw_fd(), &mut ios), 0);
        libc::cfmakeraw(&mut ios);
        assert_eq!(libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &ios), 0);
    }
    Pty { master, slave }
}

impl Pty {
    /// 端末が送ったことにする。
    pub fn terminal_sends(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).unwrap();
        self.master.flush().unwrap();
    }
}
