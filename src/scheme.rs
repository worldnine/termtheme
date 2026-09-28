//! モード 2031 — 端末の配色（ダーク／ライト）が変わったときの知らせ。
//!
//! ```text
//! CSI ? 2031 h        知らせの購読を始める（アプリ → 端末）
//! CSI ? 2031 l        購読を終える
//! CSI ? 996 n         今の配色を問い合わせる（答えは知らせと同じ形）
//! CSI ? 997 ; 1 n     ダークになった（端末 → アプリ）
//! CSI ? 997 ; 2 n     ライトになった
//! ```
//!
//! 送る側は crossterm の [`Command`] にしてある（`execute!(stdout, EnableColorSchemeUpdates)`）。
//! **受ける側は crossterm では受けられない**: crossterm 0.29 は知らせの後ろの入力を飲み込み、
//! イベントループごと止まる（`tests/crossterm_2031.rs`）。知らせは [`crate::input`] の読み手が
//! [`crate::input::Input::ColorScheme`] として渡す。
//!
//! # 購読したら、端末を手放す前に必ず外す
//!
//! 購読は端末の状態で、プロセスが終わっても残る。外さずに終わると、次に端末を使う
//! プログラム（シェル、Enter で開いた別の TUI）に知らせが届き、crossterm で読むものは
//! そこで止まる。終わるとき・子プロセスに端末を渡すとき（suspend）は
//! [`DisableColorSchemeUpdates`] を送り、戻ったら [`EnableColorSchemeUpdates`] を送り直す。

use std::fmt;

use crossterm::Command;

/// 端末の配色。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorScheme {
    Dark,
    Light,
}

impl ColorScheme {
    /// ライトか（アプリの `light: bool` と同じ向き）。
    pub fn is_light(self) -> bool {
        self == Self::Light
    }

    /// `light: bool` から。
    pub fn from_light(light: bool) -> Self {
        if light { Self::Light } else { Self::Dark }
    }

    /// 知らせ 1 つ（`CSI ? 997 ; 1 n` / `CSI ? 997 ; 2 n`、前後に余計なバイトの無いもの）を
    /// 読む。それ以外は `None`。
    pub fn from_report(seq: &[u8]) -> Option<Self> {
        match seq {
            b"\x1b[?997;1n" => Some(Self::Dark),
            b"\x1b[?997;2n" => Some(Self::Light),
            _ => None,
        }
    }
}

/// 購読を始める列。
pub const ENABLE_UPDATES: &str = "\x1b[?2031h";
/// 購読を終える列。
pub const DISABLE_UPDATES: &str = "\x1b[?2031l";
/// 今の配色を問い合わせる列。
pub const QUERY: &str = "\x1b[?996n";

/// 配色の知らせの購読を始める（`CSI ? 2031 h`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnableColorSchemeUpdates;

/// 配色の知らせの購読を終える（`CSI ? 2031 l`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisableColorSchemeUpdates;

/// 今の配色を問い合わせる（`CSI ? 996 n`）。答えは知らせと同じ形で届く。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryColorScheme;

impl Command for EnableColorSchemeUpdates {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str(ENABLE_UPDATES)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Command for DisableColorSchemeUpdates {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str(DISABLE_UPDATES)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Command for QueryColorScheme {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str(QUERY)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_read_as_dark_and_light() {
        assert_eq!(
            ColorScheme::from_report(b"\x1b[?997;1n"),
            Some(ColorScheme::Dark)
        );
        assert_eq!(
            ColorScheme::from_report(b"\x1b[?997;2n"),
            Some(ColorScheme::Light)
        );
        for other in [
            &b"\x1b[?997;3n"[..],
            b"\x1b[?997;1",
            b"\x1b[?996n",
            b"x\x1b[?997;1n",
            b"\x1b[?997;1nx",
            b"",
        ] {
            assert_eq!(ColorScheme::from_report(other), None, "{other:?}");
        }
    }

    #[test]
    fn light_round_trips_through_bool() {
        for light in [false, true] {
            assert_eq!(ColorScheme::from_light(light).is_light(), light);
        }
    }

    #[test]
    fn commands_write_the_sequences() {
        let ansi = |c: &dyn Fn(&mut String) -> fmt::Result| {
            let mut s = String::new();
            c(&mut s).unwrap();
            s
        };
        assert_eq!(
            ansi(&|s| EnableColorSchemeUpdates.write_ansi(s)),
            "\x1b[?2031h"
        );
        assert_eq!(
            ansi(&|s| DisableColorSchemeUpdates.write_ansi(s)),
            "\x1b[?2031l"
        );
        assert_eq!(ansi(&|s| QueryColorScheme.write_ansi(s)), "\x1b[?996n");
    }
}
