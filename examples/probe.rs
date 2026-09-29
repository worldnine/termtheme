//! 端末が配色をどう知らせるかを確かめる道具。herdr の中と素の端末で試すのに使う。
//!
//! ```sh
//! cargo run --example probe              # termtheme の読み手で読む
//! cargo run --example probe -- --crossterm  # crossterm で読む（比べる用）
//! ```
//!
//! 1. 起動時の判定（OSC 11）の結果と、かかった時間を出す
//! 2. モード 2031 を購読し（[`Subscription`]）、今の配色を問い合わせる（`CSI ? 996 n`）
//! 3. 知らせ・答えが届くたびに、時刻と値を出す。OS の外観（ダーク／ライト）を切り替えると
//!    届く。キーも出す（知らせの後ろのキーが消えないかを見る）
//!
//! キー: `s` 配色を問い合わせ直す / `b` 背景色を問い合わせ直す / `p` 文字色・背景色・16 色を
//! 問い合わせて答えを待つ（待つあいだに打ったキーは後で出る）/ `z` 止まる（`fg` で戻る。
//! 止まっているあいだは購読を外す）/ `q` か Ctrl+C で終わる。終わるときに購読を外し、端末を
//! 戻す。SIGTERM・SIGHUP で終わるときも購読を外す。
//!
//! `--crossterm` では、知らせを受けた後にキーが届かなくなり、poll が戻らなくなる
//! （`u` か `c` を打つと戻る）。poll が締め切りより大きく遅れたら、そう出す。

use std::io::Write;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use termtheme::background::{self, QueryBackground};
use termtheme::colors::QueryColors;
use termtheme::input::{Input, Reader};
use termtheme::scheme::{self, QueryColorScheme, Subscription};

/// poll の締め切り。これより 500 ms 以上遅れたら「止まっていた」と出す。
const TICK: Duration = Duration::from_millis(250);

fn main() -> std::io::Result<()> {
    let use_crossterm = std::env::args().skip(1).any(|a| a == "--crossterm");
    install_signal_handlers();
    let mut guard = Guard::enter()?;

    line(&format!(
        "probe: 読み手 = {}、TERM={}、TERM_PROGRAM={}、herdr の中 = {}",
        if use_crossterm {
            "crossterm"
        } else {
            "termtheme"
        },
        env("TERM"),
        env("TERM_PROGRAM"),
        if std::env::var_os("HERDR_ENV").is_some_and(|v| v == "1") {
            "はい"
        } else {
            "いいえ"
        },
    ));

    // 1. 起動時の判定。
    let start = Instant::now();
    let bg = background::query_background();
    let took = start.elapsed();
    match bg {
        Some(rgb) => line(&format!(
            "起動時の判定: {}（背景 {}、{} ms）",
            light_name(background::is_light(rgb)),
            rgb_text(rgb),
            took.as_millis()
        )),
        None => line(&format!(
            "起動時の判定: 分からない（答えなし、{} ms）→ ダークに落とす",
            took.as_millis()
        )),
    }

    // 2. 購読と問い合わせ。
    guard.scheme.start()?;
    execute!(std::io::stdout(), QueryColorScheme)?;
    line(
        "モード 2031 を購読し、今の配色を問い合わせた\
         （s: 配色 / b: 背景色 / p: 色をまとめて / z: 止まる / q: 終わる）",
    );

    // 3. 届くのを待つ。
    let mut reader = if use_crossterm {
        None
    } else {
        Some(Reader::new()?)
    };
    loop {
        let start = Instant::now();
        let ready = match &mut reader {
            Some(reader) => reader.poll(TICK)?,
            None => crossterm::event::poll(TICK)?,
        };
        let waited = start.elapsed();
        if waited > TICK + Duration::from_millis(500) {
            line(&format!(
                "poll が {} ms 戻らなかった（締め切りは {} ms）",
                waited.as_millis(),
                TICK.as_millis()
            ));
        }
        if !ready {
            continue;
        }
        let input = match &mut reader {
            Some(reader) => reader.read()?,
            None => Input::Event(crossterm::event::read()?),
        };
        match input {
            Input::ColorScheme(scheme) => line(&format!(
                "{}  配色の知らせ: {}",
                now(),
                light_name(scheme.is_light())
            )),
            Input::Background(rgb) => line(&format!(
                "{}  背景色の答え: {} → {}",
                now(),
                rgb_text(rgb),
                light_name(background::is_light(rgb))
            )),
            Input::Foreground(rgb) => line(&format!("{}  文字色の答え: {}", now(), rgb_text(rgb))),
            Input::Palette(n, rgb) => line(&format!(
                "{}  パレットの答え: {n} 番 = {}",
                now(),
                rgb_text(rgb)
            )),
            Input::Event(Event::Key(KeyEvent {
                code: KeyCode::Char('q'),
                ..
            })) => break,
            Input::Event(Event::Key(KeyEvent {
                code: KeyCode::Char('c'),
                modifiers,
                ..
            })) if modifiers.contains(KeyModifiers::CONTROL) => {
                break;
            }
            Input::Event(Event::Key(KeyEvent {
                code: KeyCode::Char('s'),
                modifiers: KeyModifiers::NONE,
                ..
            })) => {
                execute!(std::io::stdout(), QueryColorScheme)?;
                line(&format!("{}  配色を問い合わせた", now()));
            }
            Input::Event(Event::Key(KeyEvent {
                code: KeyCode::Char('b'),
                modifiers: KeyModifiers::NONE,
                ..
            })) => {
                execute!(std::io::stdout(), QueryBackground)?;
                line(&format!("{}  背景色を問い合わせた", now()));
            }
            Input::Event(Event::Key(KeyEvent {
                code: KeyCode::Char('p'),
                modifiers: KeyModifiers::NONE,
                ..
            })) => query_colors(reader.as_mut())?,
            Input::Event(Event::Key(KeyEvent {
                code: KeyCode::Char('z'),
                modifiers: KeyModifiers::NONE,
                ..
            })) => guard.stop_until_fg()?,
            Input::Event(event) => line(&format!("{}  入力: {event:?}", now())),
            other => line(&format!("{}  ほかの入力: {other:?}", now())),
        }
    }
    Ok(())
}

/// 文字色・背景色・16 色を問い合わせ、答えが揃うか 200 ms まで待って出す。crossterm で
/// 読んでいるときは送るだけ（答えはキーに化けて届く）。
fn query_colors(reader: Option<&mut Reader>) -> std::io::Result<()> {
    let query = QueryColors::ansi();
    execute!(std::io::stdout(), &query)?;
    let Some(reader) = reader else {
        line(&format!("{}  色をまとめて問い合わせた", now()));
        return Ok(());
    };
    let start = Instant::now();
    let colors = reader.wait_for_colors(&query, Duration::from_millis(200))?;
    let took = start.elapsed().as_millis();
    let text = |c: Option<background::Rgb>| c.map_or_else(|| "-".into(), rgb_text);
    line(&format!(
        "{}  色の答え（{took} ms、{}）: 文字色 {} / 背景色 {}",
        now(),
        if query.is_answered(&colors) {
            "揃った"
        } else {
            "揃わなかった"
        },
        text(colors.foreground),
        text(colors.background),
    ));
    for row in [0..8, 8..16] {
        let cells: Vec<String> = row
            .map(|n| format!("{n:>2} {}", text(colors.palette[n])))
            .collect();
        line(&format!("    {}", cells.join("  ")));
    }
    Ok(())
}

/// raw モードと購読を、終わるとき（panic でも）に必ず戻す。
struct Guard {
    scheme: Subscription,
}

impl Guard {
    fn enter() -> std::io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(Self {
            scheme: Subscription::stdout(),
        })
    }

    /// Ctrl+Z の代わり: 購読を外し、端末を戻して止まる。`fg` で戻ったら張り直し、今の配色を
    /// 問い合わせる（止まっていたあいだの切り替えを拾う）。
    fn stop_until_fg(&mut self) -> std::io::Result<()> {
        line(&format!("{}  止まる（fg で戻る）", now()));
        self.scheme.suspend()?;
        crossterm::terminal::disable_raw_mode()?;
        // SAFETY: SIGTSTP の既定の扱いで止まる。端末は戻してある。
        unsafe { libc::raise(libc::SIGTSTP) };
        crossterm::terminal::enable_raw_mode()?;
        self.scheme.resume()?;
        line(&format!(
            "{}  戻った（購読を張り直し、配色を問い合わせた）",
            now()
        ));
        Ok(())
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        // 購読を先に外す（raw モードを抜けた後に知らせが届くと、画面に出る）。
        let _ = self.scheme.stop();
        let _ = crossterm::terminal::disable_raw_mode();
        println!("購読を外して端末を戻した");
    }
}

/// SIGTERM・SIGHUP で終わるときも購読を外す（ハンドラからは Drop が走らない）。raw モードは
/// シェルが戻す。
fn install_signal_handlers() {
    extern "C" fn unsubscribe_and_die(sig: libc::c_int) {
        scheme::unsubscribe_in_signal_handler();
        // SAFETY: 既定の扱いに戻して投げ直す。
        unsafe {
            libc::signal(sig, libc::SIG_DFL);
            libc::raise(sig);
        }
    }
    let handler: extern "C" fn(libc::c_int) = unsubscribe_and_die;
    // SAFETY: ハンドラは async-signal-safe なものだけを呼ぶ。
    unsafe {
        libc::signal(libc::SIGTERM, handler as libc::sighandler_t);
        libc::signal(libc::SIGHUP, handler as libc::sighandler_t);
    }
}

/// raw モードでは改行が行頭へ戻らないので `\r\n` で終える。
fn line(text: &str) {
    let mut out = std::io::stdout();
    let _ = write!(out, "{text}\r\n");
    let _ = out.flush();
}

fn light_name(light: bool) -> &'static str {
    if light { "ライト" } else { "ダーク" }
}

fn rgb_text((r, g, b): background::Rgb) -> String {
    format!("rgb({r}, {g}, {b})")
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| "-".into())
}

/// いまの時刻（手元の時間帯、`時:分:秒.ミリ秒`）。
fn now() -> String {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = since.as_secs() as libc::time_t;
    // SAFETY: localtime_r は渡した tm に書くだけ。
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&secs, &mut tm);
        tm
    };
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        since.subsec_millis()
    )
}
