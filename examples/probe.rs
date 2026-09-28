//! 端末が配色をどう知らせるかを確かめる道具。herdr の中と素の端末で試すのに使う。
//!
//! ```sh
//! cargo run --example probe              # termtheme の読み手で読む
//! cargo run --example probe -- --crossterm  # crossterm で読む（比べる用）
//! ```
//!
//! 1. 起動時の判定（OSC 11）の結果と、かかった時間を出す
//! 2. モード 2031 を購読し、今の配色を問い合わせる（`CSI ? 996 n`）
//! 3. 知らせ・答えが届くたびに、時刻と値を出す。OS の外観（ダーク／ライト）を切り替えると
//!    届く。キーも出す（知らせの後ろのキーが消えないかを見る）
//!
//! キー: `s` 配色を問い合わせ直す / `b` 背景色を問い合わせ直す / `q` か Ctrl+C で終わる。
//! 終わるときに購読を外し、端末を戻す。
//!
//! `--crossterm` では、知らせを受けた後にキーが届かなくなり、poll が戻らなくなる
//! （`u` か `c` を打つと戻る）。poll が締め切りより大きく遅れたら、そう出す。

use std::io::Write;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use termtheme::background::{self, QueryBackground};
use termtheme::input::{Input, Reader};
use termtheme::scheme::{DisableColorSchemeUpdates, EnableColorSchemeUpdates, QueryColorScheme};

/// poll の締め切り。これより 500 ms 以上遅れたら「止まっていた」と出す。
const TICK: Duration = Duration::from_millis(250);

fn main() -> std::io::Result<()> {
    let use_crossterm = std::env::args().skip(1).any(|a| a == "--crossterm");
    let _guard = Guard::enter()?;

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
    execute!(
        std::io::stdout(),
        EnableColorSchemeUpdates,
        QueryColorScheme
    )?;
    line("モード 2031 を購読し、今の配色を問い合わせた（s: 配色 / b: 背景色 / q: 終わる）");

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
            Input::Event(event) => line(&format!("{}  入力: {event:?}", now())),
        }
    }
    Ok(())
}

/// raw モードと購読を、終わるとき（panic でも）に必ず戻す。
struct Guard;

impl Guard {
    fn enter() -> std::io::Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = execute!(std::io::stdout(), DisableColorSchemeUpdates);
        let _ = crossterm::terminal::disable_raw_mode();
        println!("購読を外して端末を戻した");
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
