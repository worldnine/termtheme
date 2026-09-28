//! 起動時の判定（[`termtheme::background::detect_light`]）を実物の pty で。問い合わせは stdout へ、
//! 答えは stdin から読むので、プロセスの fd 0 と fd 1 を pty に差し替える（fd 1 は後で戻す）。
//! fd はプロセスに 1 組なので、テストは 1 本にまとめて順に確かめる。

#![cfg(unix)]

mod common;

use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

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

/// 端末の役: 問い合わせを 1 つ読んだら、`delay` 待って `answer` を返す。読んだバイトを返す。
fn answer_once(
    master: &std::fs::File,
    delay: Duration,
    answer: &'static [u8],
) -> std::thread::JoinHandle<Vec<u8>> {
    let mut master = master.try_clone().unwrap();
    std::thread::spawn(move || {
        let mut got = Vec::new();
        let mut buf = [0u8; 64];
        while !got.ends_with(QUERY) {
            let n = master.read(&mut buf).unwrap();
            got.extend_from_slice(&buf[..n]);
        }
        std::thread::sleep(delay);
        master.write_all(answer).unwrap();
        got
    })
}

/// 読み手（プロセスに 1 つのほう）から `wait` のあいだに読めた入力。
fn read_for(wait: Duration) -> Vec<Input> {
    let until = Instant::now() + wait;
    let mut out = Vec::new();
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        if !input::poll(left).unwrap() {
            break;
        }
        out.push(input::read().unwrap());
    }
    out
}

#[test]
fn detect_light_reads_the_answer_and_hands_type_ahead_and_late_answers_to_the_reader() {
    let pty = common::open_raw(80, 24);
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

    // 1. 先打ち（答えの前の `jk` と後ろの `l`）と答えが 1 度に届く。
    let term = answer_once(
        &pty.master,
        Duration::ZERO,
        b"jk\x1b]11;rgb:ffff/ffff/ffff\x1b\\l",
    );
    let light = background::detect_light();
    let query = term.join().unwrap();

    // 2. 締め切り（150 ms）に遅れて届く答え。
    let term = answer_once(
        &pty.master,
        Duration::from_millis(400),
        b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\",
    );
    let start = Instant::now();
    let late = background::detect_light();
    let took = start.elapsed();

    // SAFETY: 退避した fd 1 を戻す（ここから下の出力はテストの出力へ）。
    unsafe {
        libc::dup2(saved_stdout, libc::STDOUT_FILENO);
        libc::close(saved_stdout);
    }

    assert_eq!(query, QUERY);
    assert_eq!(light, Some(true));
    // 先打ちは消えずに読み手から読める。
    assert_eq!(
        read_for(Duration::from_millis(200)),
        vec![key('j'), key('k'), key('l')]
    );

    assert_eq!(late, None);
    assert!(took < Duration::from_millis(300), "{took:?}");
    term.join().unwrap();
    // 遅れた答えはキーに化けず、背景色として届く。
    let got = read_for(Duration::from_millis(600));
    assert_eq!(got, vec![Input::Background((0x1e, 0x1e, 0x2e))]);
    assert_eq!(got[0].light(), Some(false));
}
