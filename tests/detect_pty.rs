//! 起動時の判定（[`termtheme::background::detect_light`]）を実物の pty で。問い合わせは stdout へ、
//! 答えは stdin から読むので、プロセスの fd 0 と fd 1 を pty に差し替える（fd 1 は後で戻す）。
//! fd はプロセスに 1 組なので、テストは 1 本にまとめて順に確かめる。
//!
//! 時間に頼らない（`tests/common` の冒頭）: 判定の締め切り（150 ms）に間に合わせたい答えは、
//! 判定を呼ぶ**前に**入力の列に載せておく（判定からは、問い合わせの直後に届いた答えと
//! 見分けがつかない）。遅れて届く答えは、判定が戻ってから送る。

#![cfg(unix)]

mod common;

use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use common::PATIENCE;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use termtheme::background;
use termtheme::input::{self, Input};

const QUERY: &[u8] = b"\x1b]11;?\x1b\\";

fn key(c: char) -> Input {
    Input::Event(Event::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::NONE,
    )))
}

/// 読み手（プロセスに 1 つのほう）から入力を `n` 個読む。外から届く SIGWINCH（herdr の
/// ペインの大きさが変わったときなど）の `Resize` は数えない。
fn read_n(n: usize) -> Vec<Input> {
    let until = Instant::now() + PATIENCE;
    let mut got = Vec::new();
    while got.len() < n {
        assert!(Instant::now() < until, "{n} 個届かない: {got:?}");
        if input::poll(Duration::from_millis(100)).unwrap() {
            let input = input::read().unwrap();
            if !matches!(input, Input::Event(Event::Resize(..))) {
                got.push(input);
            }
        }
    }
    got
}

/// 読める入力がもう無い（`Resize` は除く）。
fn assert_no_more() {
    while input::poll(Duration::ZERO).unwrap() {
        let input = input::read().unwrap();
        assert!(
            matches!(input, Input::Event(Event::Resize(..))),
            "余計な入力: {input:?}"
        );
    }
}

#[test]
fn detect_light_reads_the_answer_and_hands_type_ahead_and_late_answers_to_the_reader() {
    let mut pty = common::open_raw(80, 24);
    // SAFETY: fd 1 を退避してから、fd 0 と fd 1 を pty の slave にする。
    let saved_stdout = unsafe { libc::dup(libc::STDOUT_FILENO) };
    unsafe {
        assert_eq!(
            libc::dup2(pty.slave.as_raw_fd(), libc::STDIN_FILENO),
            libc::STDIN_FILENO
        );
        assert_eq!(
            libc::dup2(pty.slave.as_raw_fd(), libc::STDOUT_FILENO),
            libc::STDOUT_FILENO
        );
    }

    // 1. 先打ち（答えの前の `jk` と後ろの `l`）と答えが、判定の読む前に揃っている。
    pty.terminal_sends(b"jk\x1b]11;rgb:ffff/ffff/ffff\x1b\\l");
    let light = background::detect_light();

    // 2. 答えの無いまま締め切り（150 ms）を迎える。そのあいだ SIGWINCH が届き続けても
    // （herdr のペインの大きさが変わるなど）、締め切りまで待つ。読み手を先に作って
    // SIGWINCH のハンドラを入れ（入っていないと SIGWINCH は無視され、待ちを起こさない）、
    // 判定しているこのスレッドへ送り続ける。
    assert!(
        input::poll(Duration::ZERO).unwrap(),
        "先打ちが読み手に渡っている"
    );
    // SAFETY: 自分のスレッドの識別子を取るだけ。
    let me = unsafe { libc::pthread_self() };
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let winch = {
        let stop = stop.clone();
        let me = me as usize;
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                // SAFETY: 生きているスレッド（判定しているテストの本体）へ送る。
                unsafe { libc::pthread_kill(me as libc::pthread_t, libc::SIGWINCH) };
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };
    let start = Instant::now();
    let late = background::detect_light();
    let took = start.elapsed();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    winch.join().unwrap();

    // SAFETY: 退避した fd 1 を戻す（ここから下の出力はテストの出力へ）。
    unsafe {
        libc::dup2(saved_stdout, libc::STDOUT_FILENO);
        libc::close(saved_stdout);
    }

    // 問い合わせは端末へ届いている（判定 2 回ぶん）。
    assert_eq!(pty.read_output(QUERY.len() * 2), [QUERY, QUERY].concat());
    assert_eq!(light, Some(true));
    // 先打ちは消えずに読み手から読める。
    assert_eq!(read_n(3), vec![key('j'), key('k'), key('l')]);
    assert_no_more();

    assert_eq!(late, None);
    // 締め切りまで待ってから諦めた（poll の待ちは ms に切り捨てるので、少し手前まで）。
    // シグナルで起こされて途中で諦めていれば、ここが 10 ms ほどになる。
    assert!(
        took >= Duration::from_millis(100) && took < PATIENCE,
        "{took:?}"
    );
    // 締め切りに遅れた答えは、キーに化けずに背景色として届く。
    pty.terminal_sends(b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\");
    let got = read_n(1);
    assert_eq!(got, vec![Input::Background((0x1e, 0x1e, 0x2e))]);
    assert_eq!(got[0].light(), Some(false));
    assert_no_more();
}
