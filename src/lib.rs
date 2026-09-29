//! ratatui + syntect の TUI のための、ライト／ダークの配管。
//!
//! - [`background`] — 起動時に端末の背景色を問い合わせて（OSC 11）ライトかを決める
//! - [`scheme`] — モード 2031（配色が変わったときの知らせ）の購読と問い合わせ。購読の張り外しを
//!   包む [`scheme::Subscription`]（落とせば外す、子に端末を渡すあいだは外す）
//! - [`input`] — 端末の入力を読む。crossterm の `event::poll` / `event::read` の代わりで、
//!   crossterm では受けられない配色の知らせと色の答え（背景色・文字色・パレット）も渡す
//! - [`colors`] — 文字色（OSC 10）・パレット（OSC 4）の問い合わせ。答えが揃うまで、キーを
//!   捨てずに待つ（[`input::wait_for_colors`]）
//! - [`theme`] — テーマの名前か `.tmTheme` のパスを syntect の [`syntect::highlighting::Theme`]
//!   にする。ライト用・ダーク用の対と、背景からの選択
//! - [`config`] — 設定ファイルの `[theme]` の表、設定ディレクトリの解決、フラグとの重ね方
//!
//! herdr には依存しない。
//!
//! ```no_run
//! use termtheme::{background, config, theme};
//!
//! # fn main() -> std::io::Result<()> {
//! crossterm::terminal::enable_raw_mode()?;
//! // --light / --dark が無ければ端末に聞く。答えなければダーク。
//! let light = background::detect_light().unwrap_or(false);
//! let flags = config::ThemeFlags::default(); // --theme / --theme-dark / --theme-light
//! let themes = flags.over(None); // 設定ファイルの [theme] があれば Some(&pair)
//! let syntect_theme = themes.resolve(light);
//! # let _ = syntect_theme;
//! # Ok(())
//! # }
//! ```

pub mod background;
pub mod colors;
pub mod config;
#[cfg(unix)]
pub mod input;
pub mod scheme;
pub mod theme;
