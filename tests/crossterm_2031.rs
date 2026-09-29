//! crossterm 0.29 が端末の報告（モード 2031 の知らせ・OSC 11 の答え）をどう扱うかを、
//! 実物の pty で確かめる。**crossterm の今の振る舞いを写し取るテスト**で、直すものではない。
//! [`termtheme::input`] が crossterm の代わりに入力を読む理由がここにある。
//!
//! このファイルのテストはプロセスの fd 0 を pty の slave に差し替える（crossterm は fd 0 が
//! 端末ならそれを読み、ほかの fd を指せない）。fd 0 はプロセスに 1 つなので、テストは 1 本に
//! まとめて順に確かめる。
//!
//! # 時間に頼らない
//!
//! crossterm は別のスレッドで読ませ、`poll` を**頼んだときに 1 回だけ**呼ぶ（回しっぱなしに
//! すると、区間をまたいだ結果が混ざる）。送ったバイトは入力の列に全部載ったのを確かめてから
//! poll を頼むので、crossterm はそれを 1 回で読む。「止まっている」は、その poll が
//! `c` を送るまで戻らないこと（戻るはずの 100 ms を大きく過ぎても答えが無く、`c` で戻る）と、
//! 知らせの後ろのキーが 1 つも届かないことで確かめる。止まった crossterm は `read(2)` で
//! 待っているので、`c` は列に載るのを待たずに送る（載ったそばから読まれる。Linux の pty では
//! 載ったところを見られない）。
//!
//! # SIGWINCH は無視する
//!
//! crossterm は、SIGWINCH と端末の入力を同じ回に受けると `Resize` だけを返し、入力は次の
//! 入力が来るまで読まない（mio の kqueue はエッジで知らせるので、読みそこねた「読める」は
//! 二度と来ない。macOS で確かめた）。これは 2031 とは別の問題で、外から SIGWINCH が届くと
//! （テストを走らせている herdr のペインの大きさが変わるなど）このテストの入力が届かなく
//! なる。そこで crossterm の読み手を作った直後に SIGWINCH を無視にする（signal-hook の
//! ハンドラを外す）。
//!
//! crossterm の版を上げてこのテストが落ちたら、振る舞いが変わったということ。`input` の
//! 冒頭の doc と合わせて見直す。

#![cfg(unix)]

mod common;

use std::os::fd::AsRawFd;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::PATIENCE;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};

/// crossterm に poll を頼む別のスレッド。1 回頼むと、1 回答える（`Some(event)` は poll が
/// `true` で、続けて read したもの。`None` は poll が `false`）。
struct Crossterm {
    ask: mpsc::Sender<Duration>,
    answer: mpsc::Receiver<Option<Event>>,
}

impl Crossterm {
    fn start() -> Self {
        let (ask, asked) = mpsc::channel::<Duration>();
        let (answered, answer) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(timeout) = asked.recv() {
                let event = event::poll(timeout)
                    .unwrap()
                    .then(|| event::read().unwrap());
                if answered.send(event).is_err() {
                    return;
                }
            }
        });
        Self { ask, answer }
    }

    /// `poll(timeout)` を 1 回頼み、答えを `wait` まで待つ（答えが無ければ `None`）。
    fn poll(&self, timeout: Duration, wait: Duration) -> Option<Option<Event>> {
        self.ask.send(timeout).unwrap();
        self.answer.recv_timeout(wait).ok()
    }

    /// 頼んだ poll の答えを、改めて `wait` まで待つ。
    fn answer(&self, wait: Duration) -> Option<Option<Event>> {
        self.answer.recv_timeout(wait).ok()
    }

    /// イベントを `n` 個読む。大きさの変化は数えない（SIGWINCH を無視にする前に届いた分）。
    fn events(&self, n: usize) -> Vec<Event> {
        let until = Instant::now() + PATIENCE;
        let mut got = Vec::new();
        while got.len() < n {
            assert!(Instant::now() < until, "{n} 個届かない: {got:?}");
            match self.poll(Duration::from_millis(100), PATIENCE) {
                Some(Some(Event::Resize(..))) | Some(None) => {}
                Some(Some(event)) => got.push(event),
                None => panic!("poll が戻らない（読んだイベント: {got:?}）"),
            }
        }
        got
    }

    /// 読めるイベントがもう無い（大きさの変化は除く）。
    fn assert_no_more(&self) {
        loop {
            match self.poll(Duration::ZERO, PATIENCE) {
                Some(None) => return,
                Some(Some(Event::Resize(..))) => {}
                other => panic!("余計なイベント: {other:?}"),
            }
        }
    }

    /// 何も無ければ `poll(100ms)` は時間切れで戻る（止まっていないときの形）。
    fn assert_times_out(&self) {
        loop {
            match self.poll(Duration::from_millis(100), PATIENCE) {
                Some(None) => return,
                Some(Some(Event::Resize(..))) => {}
                other => panic!("時間切れで戻らない: {other:?}"),
            }
        }
    }

    /// `poll(100ms)` を頼み、それが戻らない（止まっている）ことを確かめる。戻るはずの
    /// 100 ms を大きく過ぎても答えが無ければ止まっている。止まった poll の答えは、
    /// 後で [`Crossterm::answer`] で受ける。
    fn assert_stuck(&self) {
        loop {
            match self.poll(Duration::from_millis(100), Duration::from_millis(500)) {
                None => return,
                // 無視にする前の SIGWINCH を拾って戻った。頼み直す。
                Some(Some(Event::Resize(..))) => {}
                other => panic!("poll が戻った（止まらなかった）: {other:?}"),
            }
        }
    }
}

fn key(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
}

fn alt(c: char) -> Event {
    Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT))
}

#[test]
fn crossterm_0_29_swallows_input_after_a_mode_2031_report_and_turns_osc_answers_into_keys() {
    let mut pty = common::open_raw(80, 24);
    // SAFETY: slave は開いている。fd 0 を置き換えるだけ。
    assert_eq!(
        unsafe { libc::dup2(pty.slave.as_raw_fd(), libc::STDIN_FILENO) },
        libc::STDIN_FILENO
    );
    crossterm::terminal::enable_raw_mode().unwrap();
    let crossterm = Crossterm::start();
    // crossterm の読み手を作り（初めての poll で作られ、SIGWINCH のハンドラも入る）、
    // SIGWINCH を無視にする（冒頭の「SIGWINCH は無視する」）。それまでに届いた分の
    // `Resize` は読み捨てる。
    crossterm.assert_no_more();
    // SAFETY: このプロセスの SIGWINCH の扱いを「無視」にするだけ。
    unsafe { libc::signal(libc::SIGWINCH, libc::SIG_IGN) };
    crossterm.assert_no_more();

    // 前提: 普通のキーは届き、何も無ければ poll は時間切れで戻る。
    pty.terminal_sends(b"a");
    assert_eq!(crossterm.events(1), vec![key('a')]);
    crossterm.assert_no_more();
    crossterm.assert_times_out();

    // 1. モード 2031 の知らせ（ダーク）と、その後のキー。
    //
    // crossterm は `CSI ?` で始まる列の終わりを `u`（kitty の旗の答え）と `c`（DA1 の答え）
    // でしか判定しない。`n` で終わる知らせは「続きを待つ」まま、後ろのバイトを全部同じ
    // バッファに積む。しかも入力の fd はブロッキングで、続きを待つあいだ crossterm は
    // `read(2)` で止まる: `poll(100ms)` が時間切れで戻らない（アプリのイベントループごと
    // 止まり、描き直しもタイマーも回らない）。
    pty.terminal_sends(b"\x1b[?997;1n");
    pty.terminal_sends(b"j");
    pty.terminal_sends(b"\x1b[A"); // ↑ も同じバッファに消える
    pty.terminal_sends(b"k");
    crossterm.assert_stuck();

    // `c` が来て初めて「DA1 の答え」として捨てられ、バッファが空になる。止まっていた poll は
    // ここでイベント無しで戻る — 知らせの後ろのキーも `c` 自体も届かない。止まった crossterm は
    // `read(2)` で待っているので、`c` はそのまま読まれる（入力の列に載るのは待たない）。
    pty.terminal_sends_to_waiting_reader(b"c");
    assert_eq!(crossterm.answer(PATIENCE), Some(None), "c で戻る");
    crossterm.assert_no_more();
    pty.terminal_sends(b"z");
    assert_eq!(crossterm.events(1), vec![key('z')], "その後は元どおり");
    crossterm.assert_no_more();

    // 2. ライトの知らせ（`CSI ? 997 ; 2 n`）も同じ。`u` でも抜ける。
    pty.terminal_sends(b"\x1b[?997;2n");
    pty.terminal_sends(b"x");
    crossterm.assert_stuck();
    pty.terminal_sends_to_waiting_reader(b"u");
    assert_eq!(crossterm.answer(PATIENCE), Some(None), "u で戻る");
    pty.terminal_sends(b"y");
    assert_eq!(crossterm.events(1), vec![key('y')]);
    crossterm.assert_no_more();

    // 3. OSC 11 の答え（問い合わせの締め切りに遅れて届いたもの）は、キーに化ける:
    // `ESC ]` が Alt+`]`、残りの文字がそれぞれ 1 つのキー、ST（`ESC \`）が Alt+`\`。
    pty.terminal_sends(b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\");
    let mut expected = vec![alt(']')];
    expected.extend("11;rgb:ffff/ffff/ffff".chars().map(key));
    expected.push(alt('\\'));
    assert_eq!(crossterm.events(expected.len()), expected);
    crossterm.assert_no_more();

    crossterm::terminal::disable_raw_mode().unwrap();
}
