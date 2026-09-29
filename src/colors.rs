//! 端末の色の問い合わせ — 文字色（OSC 10）・背景色（OSC 11）・パレット（OSC 4）。
//!
//! ```text
//! OSC 10 ; ? ST        文字色を問い合わせる      → OSC 10 ; rgb:RRRR/GGGG/BBBB ST
//! OSC 11 ; ? ST        背景色を問い合わせる      → OSC 11 ; rgb:… ST
//! OSC 4 ; n ; ? ST     パレットの n 番を問い合わせる → OSC 4 ; n ; rgb:… ST
//! ```
//!
//! ライトかダークかだけが要るなら [`crate::background`] で足りる。端末の実際の色で面を
//! 作るアプリ（背景に文字色や 16 色を少し混ぜるなど）は、ここで文字色と 16 色も聞く。
//!
//! 送る側は crossterm の [`Command`]（[`QueryForeground`]・[`QueryPalette`]、まとめて送る
//! [`QueryColors`]。背景色は [`crate::background::QueryBackground`]）。答えは
//! [`crate::input`] の読み手が `Input::Foreground`・`Input::Background`・`Input::Palette`
//! として渡す。
//!
//! # 問い合わせて、答えが揃うまで待つ
//!
//! 送ってから [`crate::input::wait_for_colors`] で待つ。読み手を回して答えを [`Colors`] に
//! 集め、**そのあいだに届いたキーや知らせは捨てずに取っておき**、後の `input::poll` /
//! `input::read` で届いた順に渡す（stdin を直に読むと、待つあいだに打たれたキーが消える）。
//!
//! ```no_run
//! use std::time::Duration;
//! use crossterm::execute;
//! use termtheme::colors::QueryColors;
//! use termtheme::input;
//!
//! # fn main() -> std::io::Result<()> {
//! let query = QueryColors::ansi(); // 文字色・背景色・16 色
//! execute!(std::io::stdout(), &query)?;
//! let colors = input::wait_for_colors(&query, Duration::from_millis(200))?;
//! if let (Some(fg), Some(bg), Some(ansi)) = (colors.foreground, colors.background, colors.ansi()) {
//!     // 端末の実際の色で描く
//! #   let _ = (fg, bg, ansi);
//! }
//! # Ok(())
//! # }
//! ```

use std::fmt;

use crossterm::Command;

use crate::background::{self, Rgb};

/// 文字色を問い合わせる（OSC 10）。答えは `Input::Foreground` で届く。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryForeground;

/// パレットの `n` 番（0〜255。0〜15 が 16 色）を問い合わせる（OSC 4）。答えは
/// `Input::Palette(n, rgb)` で届く。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryPalette(pub u8);

impl Command for QueryForeground {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str("\x1b]10;?\x1b\\")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Command for QueryPalette {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b]4;{};?\x1b\\", self.0)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 色をまとめて問い合わせる。送る列（[`Command`]）であり、待つものの一覧でもある
/// （[`crate::input::wait_for_colors`] に同じものを渡す）。
///
/// 列は文字色（OSC 10）・背景色（OSC 11）・パレットの番（OSC 4、書いた順）の順に、
/// 1 つずつ別の問い合わせとして書く（1 つの OSC 4 に番を並べる書き方は、答えない端末がある）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryColors {
    /// 文字色（OSC 10）。
    pub foreground: bool,
    /// 背景色（OSC 11）。
    pub background: bool,
    /// パレットの番（OSC 4）。
    pub palette: Vec<u8>,
}

impl QueryColors {
    /// 文字色・背景色・16 色（パレットの 0〜15 番）。
    pub fn ansi() -> Self {
        Self {
            foreground: true,
            background: true,
            palette: (0..16).collect(),
        }
    }

    /// 問い合わせたものの答えが `colors` に全部あるか。
    pub fn is_answered(&self, colors: &Colors) -> bool {
        (!self.foreground || colors.foreground.is_some())
            && (!self.background || colors.background.is_some())
            && self
                .palette
                .iter()
                .all(|&n| colors.palette[usize::from(n)].is_some())
    }
}

impl Command for QueryColors {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        if self.foreground {
            QueryForeground.write_ansi(f)?;
        }
        if self.background {
            background::QueryBackground.write_ansi(f)?;
        }
        for &n in &self.palette {
            QueryPalette(n).write_ansi(f)?;
        }
        Ok(())
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 集まった色の答え。答えの無かったものは `None`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Colors {
    /// 文字色（OSC 10）。
    pub foreground: Option<Rgb>,
    /// 背景色（OSC 11）。
    pub background: Option<Rgb>,
    /// パレット（OSC 4）。番で引く。
    pub palette: [Option<Rgb>; 256],
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            foreground: None,
            background: None,
            palette: [None; 256],
        }
    }
}

impl Colors {
    /// 16 色（パレットの 0〜15 番）が全部あれば、その 16 色。
    pub fn ansi(&self) -> Option<[Rgb; 16]> {
        let mut out = [(0, 0, 0); 16];
        for (o, c) in out.iter_mut().zip(&self.palette) {
            *o = (*c)?;
        }
        Some(out)
    }
}

/// OSC の答え 1 つに入っていた色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    Foreground(Rgb),
    Background(Rgb),
    Palette(u8, Rgb),
}

/// 読み終えた OSC の答え 1 つ（`ESC ] … BEL` か `ESC ] … ESC \`）から色を読む。OSC 10・11・4
/// のほかと、読めない色は読まない（空）。OSC 4 は 1 つに番と色を並べて答える端末もある
/// （`OSC 4 ; 1 ; <色> ; 2 ; <色> ST`）ので、組を全部読む。色の書き方は
/// [`background::parse_color`]。
pub(crate) fn parse_answer(osc: &[u8]) -> Vec<Answer> {
    let body = osc.strip_prefix(b"\x1b]").unwrap_or(osc);
    let body = body
        .strip_suffix(b"\x1b\\")
        .or_else(|| body.strip_suffix(b"\x07"))
        .unwrap_or(body);
    let Ok(body) = std::str::from_utf8(body) else {
        return Vec::new();
    };
    let mut fields = body.split(';');
    let color = |field: Option<&str>| field.and_then(background::parse_color);
    match fields.next() {
        Some("10") => color(fields.next())
            .map(Answer::Foreground)
            .into_iter()
            .collect(),
        Some("11") => color(fields.next())
            .map(Answer::Background)
            .into_iter()
            .collect(),
        Some("4") => {
            let mut answers = Vec::new();
            while let (Some(n), Some(c)) = (fields.next(), fields.next()) {
                if let (Ok(n), Some(c)) = (n.parse::<u8>(), background::parse_color(c)) {
                    answers.push(Answer::Palette(n, c));
                }
            }
            answers
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ansi(c: &impl Command) -> String {
        let mut s = String::new();
        c.write_ansi(&mut s).unwrap();
        s
    }

    #[test]
    fn commands_write_the_queries() {
        assert_eq!(ansi(&QueryForeground), "\x1b]10;?\x1b\\");
        assert_eq!(ansi(&QueryPalette(0)), "\x1b]4;0;?\x1b\\");
        assert_eq!(ansi(&QueryPalette(255)), "\x1b]4;255;?\x1b\\");
        let query = QueryColors {
            foreground: true,
            background: true,
            palette: vec![1, 15],
        };
        assert_eq!(
            ansi(&query),
            "\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b]4;1;?\x1b\\\x1b]4;15;?\x1b\\"
        );
        // 16 色の組は 18 の問い合わせ。何も頼まなければ何も書かない。
        assert_eq!(ansi(&QueryColors::ansi()).matches("\x1b]").count(), 18);
        assert_eq!(ansi(&QueryColors::default()), "");
    }

    #[test]
    fn answered_means_every_asked_color_is_there() {
        let query = QueryColors::ansi();
        let mut colors = Colors::default();
        assert!(!query.is_answered(&colors));
        assert!(
            QueryColors::default().is_answered(&colors),
            "何も頼んでいない"
        );
        colors.foreground = Some((1, 2, 3));
        colors.background = Some((4, 5, 6));
        for n in 0..15 {
            colors.palette[n] = Some((n as u8, 0, 0));
        }
        assert!(!query.is_answered(&colors), "15 番がまだ");
        assert_eq!(colors.ansi(), None);
        colors.palette[15] = Some((15, 0, 0));
        assert!(query.is_answered(&colors));
        assert_eq!(colors.ansi().unwrap()[15], (15, 0, 0));
        // 頼んでいないものは待たない。
        let bg_only = QueryColors {
            background: true,
            ..QueryColors::default()
        };
        assert!(bg_only.is_answered(&Colors {
            background: Some((0, 0, 0)),
            ..Colors::default()
        }));
    }

    #[test]
    fn answers_read_foreground_background_and_palette() {
        assert_eq!(
            parse_answer(b"\x1b]10;rgb:e0e0/e2e2/eaea\x1b\\"),
            vec![Answer::Foreground((0xe0, 0xe2, 0xea))]
        );
        assert_eq!(
            parse_answer(b"\x1b]11;rgb:14/16/1b\x07"),
            vec![Answer::Background((0x14, 0x16, 0x1b))]
        );
        assert_eq!(
            parse_answer(b"\x1b]4;3;rgb:cdcd/0000/0000\x1b\\"),
            vec![Answer::Palette(3, (0xcd, 0, 0))]
        );
        assert_eq!(
            parse_answer(b"\x1b]4;255;#ffffff\x07"),
            vec![Answer::Palette(255, (255, 255, 255))]
        );
        // 1 つに組を並べた答え。
        assert_eq!(
            parse_answer(b"\x1b]4;1;rgb:ff/00/00;2;rgb:00/ff/00\x1b\\"),
            vec![
                Answer::Palette(1, (255, 0, 0)),
                Answer::Palette(2, (0, 255, 0))
            ]
        );
    }

    #[test]
    fn unreadable_answers_read_as_nothing() {
        for osc in [
            &b"\x1b]10;rgb:zz/zz/zz\x1b\\"[..],
            b"\x1b]10;?\x1b\\",
            b"\x1b]11;\x07",
            // 番が 0〜255 の外・数でない・色が無い。
            b"\x1b]4;256;rgb:ff/ff/ff\x1b\\",
            b"\x1b]4;x;rgb:ff/ff/ff\x1b\\",
            b"\x1b]4;1\x1b\\",
            // 知らない OSC、UTF-8 でない答え。
            b"\x1b]12;rgb:ff/ff/ff\x1b\\",
            b"\x1b]10;\xff\xfe\x07",
        ] {
            assert_eq!(parse_answer(osc), vec![], "{osc:?}");
        }
        // 並べた組のうち、読めない組だけを落とす。
        assert_eq!(
            parse_answer(b"\x1b]4;1;rgb:zz/0/0;2;rgb:0/f/0\x07"),
            vec![Answer::Palette(2, (0, 255, 0))]
        );
    }
}
