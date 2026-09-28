//! 端末の背景色の判定（OSC 11）。
//!
//! 起動時に端末へ背景色を問い合わせ（`ESC ] 11 ; ?`）、答えの明るさ（ITU-R BT.601 の輝度）で
//! ライトかダークかを決める。答えない端末（Terminal.app など）では `None` で、呼ぶ側は
//! ダークに落とす（`detect_light().unwrap_or(false)`）。
//!
//! akapen・ashiato などの `theme.rs` の `detect_light` をここへ移したもの。
//! 振る舞いは同じで、1 つだけ足した: 待っているあいだに打たれたキー（先打ち）を捨てずに
//! [`crate::input`] の読み手へ渡す（読み手を使うアプリでは先打ちが消えない）。
//!
//! # 問い合わせ中の答えと、遅れて届く答え
//!
//! [`detect_light`] は 150 ms 待って諦める。それより遅れて届いた答えは、crossterm で入力を
//! 読むと `]`・`1`・`r`… のキーに化ける（`tests/crossterm_2031.rs`）。[`crate::input`] の
//! 読み手はこれを [`crate::input::Input::Background`] として受け取る。起動の後で配色を
//! 知り直したいとき（モード 2031 を知らない端末でフォーカスが戻ったときなど）も、
//! [`QueryBackground`] を送れば、答えは読み手から届く。

use std::fmt;

use crossterm::Command;

/// 色（赤・緑・青、各 0〜255）。
pub type Rgb = (u8, u8, u8);

/// 端末の答えを待つ時間。答えない端末ではこれだけ起動が遅れる（それが唯一の代償）。
pub const TIMEOUT: std::time::Duration = std::time::Duration::from_millis(150);

/// 問い合わせ: 「背景色を教えて」（OSC 11、ST で終わる）。
pub const QUERY: &[u8] = b"\x1b]11;?\x1b\\";

/// 答えの頭。先打ちのバイトが前にあることがあるので、どこにあっても探す。
const HEADER: &[u8] = b"\x1b]11;";

/// 端末に背景色を問い合わせ、ライトかを決める。`None` は分からない（端末でない、答えない、
/// 読めない答え）で、呼ぶ側はダークに落とす。
///
/// **raw モードに入ってから呼ぶ**（答えは stdin に届くただのバイト列で、行単位の入力の
/// ままでは改行まで溜められて読めない。読めずに終わると答えが画面に出て、終わった後の
/// シェルに残る）。イベントループが入力を読み始める前に呼ぶ。問い合わせは端末へ —
/// stdout が端末ならそこ、パイプなら `/dev/tty` — 送り、答えは stdin から読む。
///
/// 待っているあいだに打たれたキー（先打ち）は答えと同じ読み込みに入る。答えは
/// バッファのどこにあっても探すので、判定は崩れない。先打ちのバイトは捨てずに
/// [`crate::input`] の読み手へ渡す（crossterm で読むアプリでは、今までどおり消える —
/// 読み終えたバイトを stdin へ戻す手段が無い）。
#[cfg(unix)]
pub fn detect_light() -> Option<bool> {
    query_background().map(is_light)
}

/// [`detect_light`] の元: 背景色そのものを問い合わせる。
#[cfg(unix)]
pub fn query_background() -> Option<Rgb> {
    use std::io::{IsTerminal, Write};
    use std::os::fd::AsRawFd;

    if !std::io::stdin().is_terminal() {
        return None;
    }
    let mut out: Box<dyn Write> = if std::io::stdout().is_terminal() {
        Box::new(std::io::stdout())
    } else {
        Box::new(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/tty")
                .ok()?,
        )
    };
    out.write_all(QUERY).ok()?;
    out.flush().ok()?;

    let fd = std::io::stdin().as_raw_fd();
    let mut resp = Vec::new();
    let mut buf = [0u8; 64];
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            break;
        }
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: 1 つの pollfd を渡す。
        let n = unsafe {
            libc::poll(
                &mut pfd,
                1,
                deadline.saturating_duration_since(now).as_millis() as i32,
            )
        };
        if n <= 0 {
            break; // 時間切れか失敗
        }
        // libc::read で読む（std の Stdin はバッファを持つので、その後で読む者が取りこぼす）。
        // SAFETY: buf の長さまでを書かせる。
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if n <= 0 {
            break;
        }
        resp.extend_from_slice(&buf[..n as usize]);
        if response_complete(&resp) {
            break;
        }
    }
    crate::input::stash(&leftover(&resp));
    parse_osc11(&resp)
}

#[cfg(not(unix))]
pub fn detect_light() -> Option<bool> {
    None
}

#[cfg(not(unix))]
pub fn query_background() -> Option<Rgb> {
    None
}

/// 背景色を問い合わせる（OSC 11）。答えは [`crate::input::Input::Background`] で届く。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryBackground;

impl Command for QueryBackground {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str("\x1b]11;?\x1b\\")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        Ok(())
    }
}

/// OSC 11 の答えは ST（`ESC \`）か BEL（`0x07`）で終わる — ただし数えるのは頭より後ろの
/// 終端だけ（答えより前に打たれた `^G` で待ちを切り上げない）。
fn response_complete(resp: &[u8]) -> bool {
    let Some(pos) = find(resp, HEADER) else {
        return false;
    };
    let rest = &resp[pos + HEADER.len()..];
    rest.windows(2).any(|w| w == b"\x1b\\") || rest.contains(&0x07)
}

/// 答えを抜いた残り（先打ちのキー）。答えが揃っていなければ全部を残す（続きが後から
/// 届けば、読み手がつなげて 1 つの答えとして読む）。
fn leftover(resp: &[u8]) -> Vec<u8> {
    match answer_span(resp) {
        Some((start, end)) => [&resp[..start], &resp[end..]].concat(),
        None => resp.to_vec(),
    }
}

/// 揃った答え 1 つの位置（頭から終端まで、`start..end`）。
fn answer_span(resp: &[u8]) -> Option<(usize, usize)> {
    let start = find(resp, HEADER)?;
    let body = start + HEADER.len();
    let at = resp[body..].iter().position(|&b| b == 0x1b || b == 0x07)? + body;
    match resp[at] {
        0x07 => Some((start, at + 1)),
        _ if resp.get(at + 1) == Some(&b'\\') => Some((start, at + 2)),
        _ => None,
    }
}

/// `haystack` の中で `needle` が最初に現れる位置（バイト単位の `find`）。
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// OSC 11 の答えを色にする。頭の前のバイト（先打ち）と終端の後ろは読まない。
/// 色の書き方は [`parse_color`]。
pub fn parse_osc11(resp: &[u8]) -> Option<Rgb> {
    let pos = find(resp, HEADER)? + HEADER.len();
    let rest = &resp[pos..];
    let end = rest
        .iter()
        .position(|&b| b == 0x1b || b == 0x07)
        .unwrap_or(rest.len());
    parse_color(std::str::from_utf8(&rest[..end]).ok()?)
}

/// 端末の色の答えの書き方（XParseColor の一部）を読む: xterm の `rgb:RRRR/GGGG/BBBB`
/// （各 1〜4 桁）、`rgba:…`（アルファは捨てる）、`#rrggbb`。それ以外は `None`。
/// OSC 10（文字色）・OSC 4（16 色）の答えの値も同じ書き方。
pub fn parse_color(s: &str) -> Option<Rgb> {
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(hex.get(0..2)?, 16).ok()?;
        let g = u8::from_str_radix(hex.get(2..4)?, 16).ok()?;
        let b = u8::from_str_radix(hex.get(4..6)?, 16).ok()?;
        return Some((r, g, b));
    }
    let s = s.strip_prefix("rgba:").or_else(|| s.strip_prefix("rgb:"))?;
    let mut channels = s.split('/');
    Some((
        parse_hex_channel(channels.next()?)?,
        parse_hex_channel(channels.next()?)?,
        parse_hex_channel(channels.next()?)?,
    ))
}

/// 1〜4 桁の 16 進の値（XParseColor の `r` / `rr` / `rrr` / `rrrr`）。3〜4 桁は下の桁を
/// 落とし、1 桁は繰り返して 8 bit にする。
fn parse_hex_channel(s: &str) -> Option<u8> {
    match s.len() {
        1 => u8::from_str_radix(s, 16).ok().map(|v| v * 0x11),
        2 => u8::from_str_radix(s, 16).ok(),
        3 | 4 => u8::from_str_radix(s.get(..2)?, 16).ok(),
        _ => None,
    }
}

/// 見た目の明るさ（ITU-R BT.601 の輝度）が真ん中の灰色より明るければライト。
pub fn is_light((r, g, b): Rgb) -> bool {
    (0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b)) > 128.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_osc11_accepts_xterm_long_and_short_forms() {
        assert_eq!(
            parse_osc11(b"\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\"),
            Some((0x1e, 0x1e, 0x1e))
        );
        assert_eq!(
            parse_osc11(b"\x1b]11;rgb:ff/00/00\x07"),
            Some((0xff, 0x00, 0x00))
        );
        assert_eq!(
            parse_osc11(b"\x1b]11;rgba:ffff/ffff/ffff/ffff\x1b\\"),
            Some((255, 255, 255))
        );
        assert_eq!(parse_osc11(b"\x1b]11;#ffffff\x1b\\"), Some((255, 255, 255)));
        // XParseColor の 1 桁・3 桁の書き方。
        assert_eq!(
            parse_osc11(b"\x1b]11;rgb:f/0/8\x1b\\"),
            Some((0xff, 0x00, 0x88))
        );
        assert_eq!(
            parse_osc11(b"\x1b]11;rgb:fdf/246/227\x1b\\"),
            Some((0xfd, 0x24, 0x22))
        );
    }

    #[test]
    fn parse_osc11_tolerates_type_ahead_and_trailing_bytes() {
        // 待っているあいだに打たれたキーは答えの前に来る — 頭はどこにあっても探す。
        assert_eq!(
            parse_osc11(b"jjq\x1b]11;rgb:ffff/ffff/ffff\x1b\\"),
            Some((255, 255, 255))
        );
        // 終端の後ろ（同じ読み込みに入った速い打鍵）は読まない。UTF-8 でなくても。
        assert_eq!(
            parse_osc11(b"\x1b]11;rgb:0000/0000/0000\x07\xff\xfe"),
            Some((0, 0, 0))
        );
    }

    #[test]
    fn parse_osc11_rejects_garbage() {
        assert_eq!(parse_osc11(b""), None);
        assert_eq!(parse_osc11(b"garbage"), None);
        // OSC 10（文字色）の答えは背景色の答えではない。
        assert_eq!(parse_osc11(b"\x1b]10;rgb:1e1e/1e1e/1e1e\x1b\\"), None);
        assert_eq!(parse_osc11(b"\x1b]11;rgb:zz/zz/zz\x1b\\"), None);
        // 多バイト文字の混じった答えでも落ちない（写しの 3 本は `&hex[0..2]` で panic した）。
        assert_eq!(parse_osc11("\x1b]11;#あい\x1b\\".as_bytes()), None);
        assert_eq!(parse_osc11("\x1b]11;rgb:あ/0/0\x1b\\".as_bytes()), None);
    }

    #[test]
    fn response_complete_detects_terminators() {
        assert!(response_complete(b"\x1b]11;rgb:1e1e/1e1e/1e1e\x1b\\"));
        assert!(response_complete(b"\x1b]11;rgb:1e1e/1e1e/1e1e\x07"));
        assert!(!response_complete(b"\x1b]11;rgb:1e1e"));
        // 答えより前の BEL で待ちを切り上げない。先打ちの後ろの揃った答えはよい。
        assert!(!response_complete(b"\x07\x1b]11;rgb:1e1e"));
        assert!(response_complete(b"jj\x1b]11;rgb:1e1e/1e1e/1e1e\x07"));
    }

    #[test]
    fn lightness_threshold() {
        assert!(is_light((255, 255, 255)));
        assert!(is_light((253, 246, 227))); // Solarized light の背景
        assert!(!is_light((0, 0, 0)));
        assert!(!is_light((40, 42, 54))); // ダークのエディタの背景
    }

    #[test]
    fn type_ahead_around_the_answer_is_kept_for_the_reader() {
        // 答えの前と後ろの先打ちを残し、答えだけを抜く。
        assert_eq!(leftover(b"jj\x1b]11;rgb:ff/ff/ff\x1b\\k"), b"jjk");
        assert_eq!(leftover(b"\x1b]11;rgb:ff/ff/ff\x07\x1b[A"), b"\x1b[A");
        // 答えが無い・揃っていないときは全部を残す（続きは読み手がつなげる）。
        assert_eq!(leftover(b"jk"), b"jk");
        assert_eq!(leftover(b"j\x1b]11;rgb:ff"), b"j\x1b]11;rgb:ff");
        assert_eq!(leftover(b""), b"");
    }

    #[test]
    fn query_background_command_writes_the_query() {
        let mut s = String::new();
        QueryBackground.write_ansi(&mut s).unwrap();
        assert_eq!(s.as_bytes(), QUERY);
    }
}
