//! pty を使うテストの補助。
//!
//! # 時間に頼らない
//!
//! テストは負荷のある機械（別の cargo のビルド、動いている herdr のペイン）でも同じ結果に
//! なるように書く。「○ ms のあいだに届いたもの」を数えず、「届くはずのもの N 個」を
//! 長い締め切り（[`PATIENCE`]）で待つ。送ったバイトは、読む前に pty の入力の列に全部
//! 載ったことを確かめる（[`Pty::terminal_sends`]）— 読み手が 1 回で全部を読むので、
//! 読み込みの境目がどこで切れるかにも依らない。

#![allow(dead_code)]

use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{Duration, Instant};

/// 必ず起きることを待つ上限。これを超えたら壊れている（負荷で遅いだけなら十分に届く長さ）。
pub const PATIENCE: Duration = Duration::from_secs(10);

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
    /// 端末が送ったことにする。**アプリの側の入力の列に全部載るまで待って**から戻る
    /// （Linux の pty は書いたバイトを少し遅れて渡す）。読み手が読んでいない間に呼ぶこと
    /// （読まれると列が減って、待ちが終わらない）。
    pub fn terminal_sends(&mut self, bytes: &[u8]) {
        let before = self.queued();
        self.master.write_all(bytes).unwrap();
        self.master.flush().unwrap();
        let until = Instant::now() + PATIENCE;
        while self.queued() < before + bytes.len() {
            assert!(
                Instant::now() < until,
                "送ったバイトが入力の列に載らない: {bytes:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// アプリの側でまだ読まれていないバイトの数。
    pub fn queued(&self) -> usize {
        let mut n: libc::c_int = 0;
        // SAFETY: 数を書かせるだけ。
        let rc = unsafe { libc::ioctl(self.slave.as_raw_fd(), libc::FIONREAD, &mut n) };
        assert_eq!(rc, 0, "FIONREAD: {}", std::io::Error::last_os_error());
        n as usize
    }

    /// アプリが端末へ書いたバイトを `n` バイト読む（締め切りは [`PATIENCE`]）。
    pub fn read_output(&mut self, n: usize) -> Vec<u8> {
        let until = Instant::now() + PATIENCE;
        let mut got = Vec::new();
        let mut buf = [0u8; 256];
        while got.len() < n {
            let left = until.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "アプリの出力が届かない: {got:?}");
            let mut pfd = libc::pollfd {
                fd: self.master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: 1 つの pollfd を渡す。
            if unsafe { libc::poll(&mut pfd, 1, left.as_millis() as i32) } <= 0 {
                continue;
            }
            let len = (n - got.len()).min(buf.len());
            let read = self.master.read(&mut buf[..len]).unwrap();
            got.extend_from_slice(&buf[..read]);
        }
        got
    }
}
