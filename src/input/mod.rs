//! 端末の入力を読む — crossterm の `event::poll` / `event::read` の代わり。
//!
//! キー・マウス・貼り付け・フォーカス・大きさの変化は crossterm と同じ
//! [`crossterm::event::Event`] で渡し、それに加えて端末の報告 — 配色の知らせ
//! （[`Input::ColorScheme`]、モード 2031）と背景色の答え（[`Input::Background`]、OSC 11）—
//! を渡す。
//!
//! # なぜ crossterm で読まないか
//!
//! crossterm 0.29 は、モード 2031 の知らせ（`CSI ? 997 ; 1 n`）を受けると、**後ろの入力を
//! `u` か `c` のバイトが来るまで全部飲み込む**。しかもそのあいだ `event::poll(timeout)` は
//! 時間切れで戻らず `read(2)` で止まる — アプリのイベントループごと止まり、描き直しも
//! タイマーも回らない。crossterm の読み手（`parse_event`）は `CSI ?` で始まる列の終わりを
//! `u`（kitty の旗の答え）と `c`（DA1 の答え）でしか見ないからで、読み手は `pub(crate)` なので
//! 外から直せない。遅れて届いた OSC 11 の答えは Alt+`]`・`1`・`1`・`;`・`r`… のキーに化ける。
//! どちらも pty で確かめてある（`tests/crossterm_2031.rs`）。
//!
//! もう 1 つ、crossterm は SIGWINCH と入力を同じ回に受けると `Resize` だけを返し、入力は
//! 次の入力が来るまで読まない（mio の kqueue はエッジで知らせるので、読みそこねた「読める」は
//! 二度と来ない。macOS で確かめた）。大きさを変えた瞬間に打ったキーが遅れる。この読み手は
//! 待つたびに端末と SIGWINCH の両方を見る（`tests/reader_resize.rs`）。
//!
//! そこで入力を crossterm より前で読む。crossterm の読み手を写し（`parse.rs`、MIT）、
//! 端末の報告を読むところだけ直した。キーの読み方は crossterm と同じ。
//!
//! # 使い方
//!
//! イベントループの `event::poll` / `event::read` をこちらに置き換え、`match` の外側に
//! 1 段足す:
//!
//! ```no_run
//! use std::time::Duration;
//! use termtheme::input::{self, Input};
//!
//! # fn main() -> std::io::Result<()> {
//! loop {
//!     if input::poll(Duration::from_millis(100))? {
//!         match input::read()? {
//!             Input::Event(event) => { /* 今までの crossterm の Event の腕 */ }
//!             other => {
//!                 if let Some(light) = other.light() {
//!                     // テーマを作り直して描き直す
//!                 }
//!             }
//!         }
//!     }
//! }
//! # }
//! ```
//!
//! 知らせを受けるには、端末に購読を頼む（[`crate::scheme::EnableColorSchemeUpdates`]）。
//!
//! # 決まりごと
//!
//! - **crossterm の `event::poll` / `event::read` と混ぜない。** 読み手が 2 つになると同じ
//!   端末を取り合い、crossterm が読んだ分はこちらに届かない（知らせを crossterm が読めば、
//!   そこで止まる）。
//! - crossterm の `cursor::position()` も crossterm の読み手を動かす（その間に届いたキーは
//!   crossterm の側に残って消え、知らせが届けばそこで止まる）。ratatui の `Terminal::clear()`
//!   はカーソル位置を守るためにこれを呼ぶ。全画面のアプリで画面を消して描き直したいときは
//!   `terminal.resize(area)` を使う（ratatui 0.30 の全画面では、消して裏のバッファを空に
//!   するだけで、カーソル位置は聞かない。大きさが変わったときの自動の描き直しも同じ道）。
//!   あるいは backend の `get_cursor_position` を crossterm に問い合わせない形にする。
//! - 端末は stdin（端末なら）か `/dev/tty` を読む（crossterm と同じ選び方）。
//! - 入力を読むのは [`poll`] / [`read`] の中だけ（別のスレッドは立てない）。子プロセスに端末を
//!   渡しているあいだは呼ばなければ、取り合わない（crossterm と同じ）。

mod parse;

use std::collections::VecDeque;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crossterm::event::Event;

use crate::background::{self, Rgb};
use crate::scheme::ColorScheme;
use parse::{InternalEvent, Parser};

/// 読んだ 1 つの入力。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// crossterm と同じ入力（キー・マウス・貼り付け・フォーカス・大きさの変化）。
    Event(Event),
    /// 端末の配色が変わった（モード 2031 の知らせ）か、`CSI ? 996 n` への答え。
    ColorScheme(ColorScheme),
    /// OSC 11 の答え（背景色）。[`crate::background::QueryBackground`] への答えや、
    /// 起動時の判定の締め切りに遅れて届いた答え。
    Background(Rgb),
}

impl Input {
    /// 配色についての入力なら、ライトか（[`Input::Background`] は背景の明るさで決める）。
    /// ほかは `None`。
    pub fn light(&self) -> Option<bool> {
        match self {
            Self::ColorScheme(scheme) => Some(scheme.is_light()),
            Self::Background(rgb) => Some(background::is_light(*rgb)),
            Self::Event(_) => None,
        }
    }
}

/// 1 回の `read(2)` で読む量（crossterm と同じ）。
const BUFFER_SIZE: usize = 1024;

/// 端末の入力の読み手。プロセスに 1 つで足りるなら [`poll`] / [`read`] を使う。
pub struct Reader {
    tty: Tty,
    parser: Parser,
    winch: Winch,
    /// アプリへ渡す入力（端末の内部の報告は落としてある）。
    inputs: VecDeque<Input>,
    buffer: Box<[u8; BUFFER_SIZE]>,
}

/// 読む端末。stdin は借りているだけで閉じない。
enum Tty {
    Stdin,
    Owned(OwnedFd),
}

impl Tty {
    fn raw(&self) -> RawFd {
        match self {
            Self::Stdin => libc::STDIN_FILENO,
            Self::Owned(fd) => fd.as_raw_fd(),
        }
    }
}

impl Reader {
    /// stdin が端末ならそれを、でなければ `/dev/tty` を読む（crossterm と同じ選び方）。
    pub fn new() -> io::Result<Self> {
        // SAFETY: fd 0 を調べるだけ。
        if unsafe { libc::isatty(libc::STDIN_FILENO) } == 1 {
            return Self::with_tty(Tty::Stdin);
        }
        let tty = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")?;
        Self::with_tty(Tty::Owned(tty.into()))
    }

    /// 開いてある端末を読む（pty で試すときなど）。
    pub fn from_fd(fd: OwnedFd) -> io::Result<Self> {
        Self::with_tty(Tty::Owned(fd))
    }

    fn with_tty(tty: Tty) -> io::Result<Self> {
        Ok(Self {
            tty,
            parser: Parser::default(),
            winch: Winch::new()?,
            inputs: VecDeque::new(),
            buffer: Box::new([0; BUFFER_SIZE]),
        })
    }

    /// `timeout` まで待って、読める入力があるか。`true` なら続く [`Reader::read`] は待たない。
    ///
    /// crossterm と違い、列の途中で入力が途切れても `timeout` で必ず戻る。
    pub fn poll(&mut self, timeout: Duration) -> io::Result<bool> {
        self.take_stash();
        if !self.inputs.is_empty() {
            return Ok(true);
        }
        let deadline = Instant::now().checked_add(timeout);
        loop {
            let left = deadline.map(|d| d.saturating_duration_since(Instant::now()));
            let ready = wait(self.tty.raw(), self.winch.rx.as_raw_fd(), left)?;
            if ready.winch && self.winch.drain() {
                let (columns, rows) = size(self.tty.raw())?;
                self.parser
                    .push_event(InternalEvent::Event(Event::Resize(columns, rows)));
                self.collect();
            }
            if ready.tty {
                self.fill()?;
            }
            if !self.inputs.is_empty() {
                return Ok(true);
            }
            if deadline.is_some_and(|d| Instant::now() >= d) {
                return Ok(false);
            }
        }
    }

    /// 入力を 1 つ読む。無ければ届くまで待つ。
    pub fn read(&mut self) -> io::Result<Input> {
        loop {
            if let Some(input) = self.inputs.pop_front() {
                return Ok(input);
            }
            self.poll(Duration::MAX)?;
        }
    }

    /// 起動時の判定（[`crate::background`]）が読んで残したバイト（先打ち）を先に読む。
    /// 判定は stdin を読むので、引き取るのも stdin を読む読み手だけ。
    fn take_stash(&mut self) {
        if !matches!(self.tty, Tty::Stdin) {
            return;
        }
        let stash = take_stash();
        if !stash.is_empty() {
            self.parser.advance(&stash, false);
            self.collect();
        }
    }

    /// 端末から 1 回読む（`poll` で読めると分かってから呼ぶ）。
    fn fill(&mut self) -> io::Result<()> {
        let n = loop {
            // SAFETY: buffer の長さまでを書かせる。
            let n = unsafe {
                libc::read(
                    self.tty.raw(),
                    self.buffer.as_mut_ptr() as *mut libc::c_void,
                    BUFFER_SIZE,
                )
            };
            if n >= 0 {
                break n as usize;
            }
            let err = io::Error::last_os_error();
            match err.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => return Ok(()),
                _ => return Err(err),
            }
        };
        if n == 0 {
            // 端末が閉じた（pty の向こうが居なくなった）。読み続けると空回りする。
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "端末の入力が閉じた",
            ));
        }
        self.parser.advance(&self.buffer[..n], n == BUFFER_SIZE);
        self.collect();
        Ok(())
    }

    /// 読み終えた列のうち、アプリへ渡すものを移す（カーソル位置・DA1 などは落とす）。
    fn collect(&mut self) {
        while let Some(event) = self.parser.next() {
            let input = match event {
                InternalEvent::Event(event) => Input::Event(event),
                InternalEvent::ColorScheme(scheme) => Input::ColorScheme(scheme),
                InternalEvent::Background(rgb) => Input::Background(rgb),
                InternalEvent::CursorPosition(..)
                | InternalEvent::KeyboardEnhancementFlags(_)
                | InternalEvent::PrimaryDeviceAttributes
                | InternalEvent::Ignored => continue,
            };
            self.inputs.push_back(input);
        }
    }
}

/// プロセスに 1 つの読み手（crossterm の `event::poll` / `event::read` と同じ持ち方）。
static READER: Mutex<Option<Reader>> = Mutex::new(None);

fn with_reader<T>(f: impl FnOnce(&mut Reader) -> io::Result<T>) -> io::Result<T> {
    let mut reader = READER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if reader.is_none() {
        *reader = Some(Reader::new()?);
    }
    f(reader.as_mut().expect("作ったばかり"))
}

/// crossterm の `event::poll` の代わり。初めて呼ばれたときに読み手（[`Reader::new`]）を作る。
pub fn poll(timeout: Duration) -> io::Result<bool> {
    with_reader(|reader| reader.poll(timeout))
}

/// crossterm の `event::read` の代わり。
pub fn read() -> io::Result<Input> {
    with_reader(|reader| reader.read())
}

/// 起動時の判定が読んだが答えではなかったバイト（先打ち）。次に読み手が読む。
static STASH: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// 読み手へ渡すバイトを預ける（[`crate::background::query_background`] から）。
pub(crate) fn stash(bytes: &[u8]) {
    if !bytes.is_empty() {
        STASH
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .extend_from_slice(bytes);
    }
}

fn take_stash() -> Vec<u8> {
    std::mem::take(
        &mut *STASH
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}

/// 端末の大きさ。読んでいる端末に聞き、だめなら crossterm に聞く（`/dev/tty` か stdout）。
fn size(fd: RawFd) -> io::Result<(u16, u16)> {
    let mut ws = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: winsize を書かせるだけ。
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_col > 0 {
        return Ok((ws.ws_col, ws.ws_row));
    }
    crossterm::terminal::size()
}

/// SIGWINCH を受けたら 1 バイト書かれる self-pipe（signal-hook。crossterm も同じものを使う）。
struct Winch {
    id: signal_hook::SigId,
    rx: UnixStream,
}

impl Winch {
    fn new() -> io::Result<Self> {
        let (rx, tx) = UnixStream::pair()?;
        rx.set_nonblocking(true)?;
        // 書く側は signal-hook が持ち、登録を外すときに閉じる。
        let id = signal_hook::low_level::pipe::register(libc::SIGWINCH, tx)?;
        Ok(Self { id, rx })
    }

    /// 溜まった知らせを読み捨てる。1 つでもあれば `true`。
    fn drain(&mut self) -> bool {
        use std::io::Read;
        let mut any = false;
        let mut buf = [0u8; 64];
        while let Ok(n) = (&self.rx).read(&mut buf) {
            if n == 0 {
                break;
            }
            any = true;
        }
        any
    }
}

impl Drop for Winch {
    fn drop(&mut self) {
        signal_hook::low_level::unregister(self.id);
    }
}

/// [`wait`] の結果: どちらが読めるか。
struct Ready {
    tty: bool,
    winch: bool,
}

/// 端末か SIGWINCH の pipe が読めるようになるまで、`timeout` まで待つ（`None` は無期限）。
/// シグナルで起こされたら何も読めないまま戻る（呼ぶ側が締め切りを見直す）。
#[cfg(not(target_os = "macos"))]
fn wait(tty: RawFd, winch: RawFd, timeout: Option<Duration>) -> io::Result<Ready> {
    let mut fds = [
        libc::pollfd {
            fd: tty,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: winch,
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    let ms = timeout.map_or(-1, |t| {
        i32::try_from(t.as_micros().div_ceil(1000)).unwrap_or(i32::MAX)
    });
    // SAFETY: 2 つの pollfd を渡す。
    if unsafe { libc::poll(fds.as_mut_ptr(), 2, ms) } < 0 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            return Ok(Ready {
                tty: false,
                winch: false,
            });
        }
        return Err(err);
    }
    if fds[0].revents & libc::POLLNVAL != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "端末の fd を poll できない",
        ));
    }
    Ok(Ready {
        // 閉じた（POLLHUP）・壊れた（POLLERR）も「読める」にして、read に理由を言わせる。
        tty: fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0,
        winch: fds[1].revents & libc::POLLIN != 0,
    })
}

/// macOS は select で待つ。macOS の poll(2) は `/dev/tty` を受け付けない（POLLNVAL）。
#[cfg(target_os = "macos")]
fn wait(tty: RawFd, winch: RawFd, timeout: Option<Duration>) -> io::Result<Ready> {
    // SAFETY: fd_set は 0 で初期化してから FD_ZERO / FD_SET で組む。
    let mut set: libc::fd_set = unsafe { std::mem::zeroed() };
    unsafe {
        libc::FD_ZERO(&mut set);
        libc::FD_SET(tty, &mut set);
        libc::FD_SET(winch, &mut set);
    }
    let mut tv = timeout.map(|t| libc::timeval {
        tv_sec: t.as_secs().min(i32::MAX as u64) as libc::time_t,
        tv_usec: t.subsec_micros() as libc::suseconds_t,
    });
    let tv_ptr = tv
        .as_mut()
        .map_or(std::ptr::null_mut(), |tv| tv as *mut libc::timeval);
    // SAFETY: 読むほうの fd_set と timeval（か null）を渡す。
    let n = unsafe {
        libc::select(
            tty.max(winch) + 1,
            &mut set,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            tv_ptr,
        )
    };
    if n < 0 {
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            return Ok(Ready {
                tty: false,
                winch: false,
            });
        }
        return Err(err);
    }
    // SAFETY: select が書き戻した fd_set を調べるだけ。
    Ok(unsafe {
        Ready {
            tty: libc::FD_ISSET(tty, &set),
            winch: libc::FD_ISSET(winch, &set),
        }
    })
}
