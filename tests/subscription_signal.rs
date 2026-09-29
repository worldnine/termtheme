//! シグナルハンドラから購読を外す（[`termtheme::scheme::unsubscribe_in_signal_handler`]）。
//! 書き先の fd はプロセスに 1 つの状態なので、別のプロセス（このファイル）で 1 本にまとめて
//! 順に確かめる。
//!
//! 時間に頼らない: 書き先はソケットの組にし、書いたバイトは同じスレッドで順に積まれる。
//! 「何も書かない」は、ハンドラを走らせた直後に目印（`|`）を書いて、目印の前に何も無いことで
//! 確かめる（`raise` はハンドラを走らせ終えてから戻る）。

#![cfg(unix)]

use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use termtheme::scheme::{Subscription, unsubscribe_in_signal_handler};

/// SIGTERM などで終わるアプリのハンドラの形（ここでは死なずに戻る）。
extern "C" fn on_signal(_: libc::c_int) {
    unsubscribe_in_signal_handler();
}

const ON: &str = "\x1b[?2031h";
const OFF: &str = "\x1b[?2031l";
const ASK: &str = "\x1b[?996n";

#[test]
fn the_signal_handler_unsubscribes_only_while_subscribed() {
    let (tx, mut rx) = UnixStream::pair().unwrap();
    // 壊れていても止まらずに落ちる（書き手が残って読み終わらない、など）。
    rx.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut marker = tx.try_clone().unwrap();
    let mut scheme = Subscription::from_fd(OwnedFd::from(tx));
    let handler: extern "C" fn(libc::c_int) = on_signal;
    // SAFETY: on_signal はハンドラとして正しい形（async-signal-safe なものだけを呼ぶ）。
    unsafe { libc::signal(libc::SIGUSR1, handler as libc::sighandler_t) };
    // 目印の書き手は closure に移す（closure を落とせば、書き手が居なくなる）。
    let mut signal_then_mark = move || {
        // SAFETY: 自分のスレッドへ送り、ハンドラが走り終えてから戻る。
        assert_eq!(unsafe { libc::raise(libc::SIGUSR1) }, 0);
        marker.write_all(b"|").unwrap();
    };

    signal_then_mark(); // 張る前: 書かない
    scheme.start().unwrap();
    signal_then_mark(); // 張っている: 外す
    scheme.suspend().unwrap();
    signal_then_mark(); // 子に渡している: 書かない（子の端末の購読を外さない）
    scheme.resume().unwrap();
    signal_then_mark(); // 張り直した: 外す
    drop(scheme);
    signal_then_mark(); // 落とした後: 書かない
    assert!(!unsubscribe_in_signal_handler());
    drop(signal_then_mark);

    // 書き手が全部居なくなったので、読み切ったところで終わる。
    let mut got = String::new();
    rx.read_to_string(&mut got).unwrap();
    assert_eq!(
        got,
        [
            "|", ON, OFF, // ハンドラ
            "|", OFF, // suspend
            "|", ON, ASK, OFF, // ハンドラ
            "|", OFF, // Drop
            "|",
        ]
        .concat()
    );
}
