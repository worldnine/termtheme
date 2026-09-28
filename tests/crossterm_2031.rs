//! crossterm 0.29 が端末の報告（モード 2031 の知らせ・OSC 11 の答え）をどう扱うかを、
//! 実物の pty で確かめる。**crossterm の今の振る舞いを写し取るテスト**で、直すものではない。
//! [`termtheme::input`] が crossterm の代わりに入力を読む理由がここにある。
//!
//! このファイルのテストはプロセスの fd 0 を pty の slave に差し替える（crossterm は fd 0 が
//! 端末ならそれを読み、ほかの fd を指せない）。fd 0 はプロセスに 1 つなので、テストは 1 本に
//! まとめて順に確かめる。
//!
//! crossterm の版を上げてこのテストが落ちたら、振る舞いが変わったということ。`input` の
//! 冒頭の doc と合わせて見直す。

#![cfg(unix)]

use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};

/// pty を開き、slave を fd 0 に差し替える。返すのは端末側（master）。
fn stdin_on_a_pty() -> std::fs::File {
    let (mut master, mut slave) = (-1, -1);
    let mut size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
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
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    // SAFETY: slave は開いている。fd 0 を置き換えるだけ。
    assert_eq!(
        unsafe { libc::dup2(slave.as_raw_fd(), libc::STDIN_FILENO) },
        libc::STDIN_FILENO
    );
    // SAFETY: 同上（master の持ち主になる）。
    unsafe { std::fs::File::from_raw_fd(master) }
}

/// 端末が送ったことにする。
fn terminal_sends(master: &mut std::fs::File, bytes: &[u8]) {
    master.write_all(bytes).unwrap();
    master.flush().unwrap();
}

/// crossterm の `poll(100ms)` 1 回の結果。
struct Polled {
    /// `true` なら続けて `read()` した結果。
    event: Option<Event>,
    /// `poll` が戻るまでにかかった時間。
    took: Duration,
}

/// 別スレッドで crossterm の `poll(100ms)` → `read()` を回し、1 回ごとの結果を流す。
/// crossterm の読み手が止まっても、テストは止まらずに「戻ってこない」を観測できる。
fn crossterm_polls() -> mpsc::Receiver<Polled> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        loop {
            let start = Instant::now();
            let ready = match event::poll(Duration::from_millis(100)) {
                Ok(ready) => ready,
                Err(_) => return,
            };
            let took = start.elapsed();
            let event = ready.then(|| event::read().unwrap());
            if tx.send(Polled { event, took }).is_err() {
                return;
            }
        }
    });
    rx
}

/// `wait` のあいだに戻ってきた poll をまとめたもの。
struct Collected {
    events: Vec<Event>,
    /// 戻ってきた poll の数。100 ms の poll なら、止まっていない限り 400 ms で数回は戻る。
    polls: usize,
    /// いちばん長くかかった poll。
    longest: Duration,
}

/// `wait` のあいだに戻ってきた poll の結果を集める。
fn collect(rx: &mpsc::Receiver<Polled>, wait: Duration) -> Collected {
    let until = Instant::now() + wait;
    let mut c = Collected {
        events: Vec::new(),
        polls: 0,
        longest: Duration::ZERO,
    };
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(p) => {
                c.polls += 1;
                c.longest = c.longest.max(p.took);
                c.events.extend(p.event);
            }
            Err(_) => break,
        }
    }
    c
}

fn key(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}

fn alt(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT))
}

#[test]
fn crossterm_0_29_swallows_input_after_a_mode_2031_report_and_turns_osc_answers_into_keys() {
    let mut master = stdin_on_a_pty();
    crossterm::terminal::enable_raw_mode().unwrap();
    let rx = crossterm_polls();
    let settle = Duration::from_millis(400);

    // 前提: 普通のキーは届き、poll は 100 ms ごとに戻ってくる。
    terminal_sends(&mut master, b"a");
    let c = collect(&rx, settle);
    assert_eq!(c.events, vec![key('a')]);
    assert!(
        c.polls >= 2 && c.longest < Duration::from_millis(300),
        "{} {:?}",
        c.polls,
        c.longest
    );

    // 1. モード 2031 の知らせ（ダーク）と、その後のキー。
    //
    // crossterm は `CSI ?` で始まる列の終わりを `u`（kitty の旗の答え）と `c`（DA1 の答え）
    // でしか判定しない。`n` で終わる知らせは「続きを待つ」まま、後ろのバイトを全部同じ
    // バッファに積む。しかも入力の fd はブロッキングで、続きを待つあいだ crossterm は
    // `read(2)` で止まる: `poll(100ms)` が時間切れで戻らない（アプリのイベントループごと
    // 止まり、描き直しもタイマーも回らない）。
    terminal_sends(&mut master, b"\x1b[?997;1n");
    terminal_sends(&mut master, b"j");
    terminal_sends(&mut master, b"\x1b[A"); // ↑ も同じバッファに消える
    terminal_sends(&mut master, b"k");
    let c = collect(&rx, settle);
    assert_eq!(c.events, vec![], "知らせの後のキーが届かない");
    assert_eq!(c.polls, 0, "poll が 1 度も戻らない（止まっている）");

    // `c` が来て初めて「DA1 の答え」として捨てられ、バッファが空になる。`c` 自体も消える。
    // 止まっていた poll はここで（イベント無しで）戻る。
    terminal_sends(&mut master, b"c");
    let c = collect(&rx, settle);
    assert_eq!(c.events, vec![]);
    assert!(
        c.longest > settle,
        "止まっていた poll の長さ: {:?}",
        c.longest
    );
    terminal_sends(&mut master, b"z");
    assert_eq!(
        collect(&rx, settle).events,
        vec![key('z')],
        "その後は元どおり"
    );

    // 2. ライトの知らせ（`CSI ? 997 ; 2 n`）も同じ。`u` でも抜ける。
    terminal_sends(&mut master, b"\x1b[?997;2n");
    terminal_sends(&mut master, b"x");
    let c = collect(&rx, settle);
    assert_eq!((c.events, c.polls), (vec![], 0));
    terminal_sends(&mut master, b"u");
    terminal_sends(&mut master, b"y");
    assert_eq!(collect(&rx, settle).events, vec![key('y')]);

    // 3. OSC 11 の答え（問い合わせの締め切りに遅れて届いたもの）は、キーに化ける:
    // `ESC ]` が Alt+`]`、残りの文字がそれぞれ 1 つのキー、ST（`ESC \`）が Alt+`\`。
    terminal_sends(&mut master, b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\");
    let mut expected = vec![alt(']')];
    expected.extend("11;rgb:ffff/ffff/ffff".chars().map(key));
    expected.push(alt('\\'));
    assert_eq!(collect(&rx, settle).events, expected);

    crossterm::terminal::disable_raw_mode().unwrap();
}
