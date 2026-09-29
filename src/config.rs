//! 設定ファイルの `[theme]` の表と、その周り。
//!
//! ```toml
//! [theme]
//! dark  = "Catppuccin Mocha"     # two-face の名前か .tmTheme のパス
//! light = "~/themes/day.tmTheme"
//! ```
//!
//! - [`ThemeSection`] — アプリの設定の構造体に埋め込む serde の型。**知らないキーは断る**
//!   （`[theme] drak = …` のタイプミスを黙って無視しない）
//! - [`ThemeSection::into_pair`] — 空・空白だけの値は書いていないのと同じにし、`~/` を
//!   展開する（`.tmTheme` のパス用）。その 2 つは [`non_blank`]・[`expand_home`] として
//!   `[theme]` の外のキーにも使える
//! - [`config_dir`] — 設定ディレクトリ: `$XDG_CONFIG_HOME/<アプリ名>`、無ければ
//!   `~/.config/<アプリ名>`（空の `XDG_CONFIG_HOME` は無いのと同じ）
//! - [`ThemeFlags::over`] — フラグとの重ね方: `--theme`（両側）> `--theme-dark` /
//!   `--theme-light` > 設定ファイル > 既定
//!
//! **どれも実環境を読まない。** 環境変数は引く関数で、ホームはパスで受け取る（テストで
//! 注入する）。ファイルを読むのもアプリの仕事で、ここは中身を受け取るだけ。
//!
//! akapen・ashiato などの `config_file.rs`（`[theme]` の部分と `user_dir`）と、
//! `config.rs` / `main.rs` のテーマの重ね方をここへ移したもの。

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::theme::ThemePair;

/// 設定ファイルの `[theme]` の表そのもの。アプリの設定の型に
/// `theme: Option<ThemeSection>` と埋め込む。
///
/// 値は書いたまま。空・空白を落とし `~/` を展開するのは [`ThemeSection::into_pair`]。
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemeSection {
    /// `[theme] dark`（`--theme-dark` の下の層）。
    pub dark: Option<String>,
    /// `[theme] light`（`--theme-light` の下の層）。
    pub light: Option<String>,
}

impl ThemeSection {
    /// 値を整えて対にする: 空・空白だけは書いていないのと同じ、`~/` で始まれば `home` の
    /// 下に展開する（`home` が無ければそのまま）。
    pub fn into_pair(self, home: Option<&Path>) -> ThemePair {
        ThemePair {
            dark: non_blank(self.dark).map(|v| expand_home(v, home)),
            light: non_blank(self.light).map(|v| expand_home(v, home)),
        }
    }
}

/// 空・空白だけは書いていないのと同じ。
///
/// `[theme]` の外のキーにも使ってよい（akapen は `semantic_cmd` などの設定の値にも使う）。
/// そのために公開してある — `[theme]` のための内側の関数にしない。
pub fn non_blank(value: Option<String>) -> Option<String> {
    value.filter(|v| !v.trim().is_empty())
}

/// `~/` で始まるなら `home` の下に展開する。`home` が無ければそのまま。
/// `~` だけや `~user/` は展開しない。
pub fn expand_home(value: String, home: Option<&Path>) -> String {
    match (value.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest).to_string_lossy().into_owned(),
        _ => value,
    }
}

/// アプリの設定ディレクトリ: `$XDG_CONFIG_HOME/<app>`、無ければ `$HOME/.config/<app>`。
///
/// `env` は環境変数 1 つを引く関数（本番は `|name| std::env::var_os(name)`）。空の
/// `XDG_CONFIG_HOME` は無いのと同じ。`HOME` も無ければ `None`（置き場が決まらない =
/// 何も置いていない）。
pub fn config_dir(app: &str, env: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let base = match env("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(env("HOME")?).join(".config"),
    };
    Some(base.join(app))
}

/// コマンドラインのテーマのフラグ。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemeFlags {
    /// `--theme`: **両側を上書きする**（1 本だったころの意味のまま、`--theme-dark` /
    /// `--theme-light` より強い）。
    pub both: Option<String>,
    /// `--theme-dark`
    pub dark: Option<String>,
    /// `--theme-light`
    pub light: Option<String>,
}

impl ThemeFlags {
    /// フラグを設定ファイルの対（[`ThemeSection::into_pair`]）の上に重ねる。各側は
    /// `--theme` > その側のフラグ > 設定ファイル > 既定（`None`）。フラグの値は
    /// 渡されたまま使う（空・空白を落とすのは設定ファイルの値だけ）。
    pub fn over(self, file: Option<&ThemePair>) -> ThemePair {
        let file = file.cloned().unwrap_or_default();
        ThemePair {
            dark: self.both.clone().or(self.dark).or(file.dark),
            light: self.both.or(self.light).or(file.light),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// アプリの設定ファイルの形（`[theme]` を埋め込み、ほかのキーも持つ）。
    #[derive(Debug, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct AppFile {
        #[allow(dead_code)]
        semantic_cmd: Option<String>,
        theme: Option<ThemeSection>,
    }

    fn parse(text: &str) -> Result<ThemePair, String> {
        let file: AppFile = toml::from_str(text).map_err(|e| e.to_string())?;
        Ok(file
            .theme
            .unwrap_or_default()
            .into_pair(Some(Path::new("/home/u"))))
    }

    fn pair(dark: Option<&str>, light: Option<&str>) -> ThemePair {
        ThemePair {
            dark: dark.map(String::from),
            light: light.map(String::from),
        }
    }

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |name| {
            pairs
                .iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn the_directory_is_xdg_config_home_else_dot_config_under_home() {
        assert_eq!(
            config_dir(
                "akapen",
                env(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/u")])
            ),
            Some(PathBuf::from("/xdg/akapen"))
        );
        // 空の XDG_CONFIG_HOME は無いのと同じ。
        assert_eq!(
            config_dir(
                "ashiato",
                env(&[("XDG_CONFIG_HOME", ""), ("HOME", "/home/u")])
            ),
            Some(PathBuf::from("/home/u/.config/ashiato"))
        );
        assert_eq!(
            config_dir("probe", env(&[("HOME", "/home/u")])),
            Some(PathBuf::from("/home/u/.config/probe"))
        );
        assert_eq!(config_dir("probe", env(&[])), None, "置き場が決まらない");
    }

    #[test]
    fn an_absent_or_empty_table_sets_nothing() {
        assert_eq!(parse("").unwrap(), ThemePair::default());
        assert_eq!(parse("[theme]\n").unwrap(), ThemePair::default());
    }

    #[test]
    fn both_keys_read_and_either_can_be_left_out() {
        assert_eq!(
            parse("[theme]\ndark = \"Catppuccin Mocha\"\nlight = \"Catppuccin Latte\"\n").unwrap(),
            pair(Some("Catppuccin Mocha"), Some("Catppuccin Latte"))
        );
        assert_eq!(
            parse("[theme]\nlight = \"Catppuccin Latte\"\n").unwrap(),
            pair(None, Some("Catppuccin Latte"))
        );
    }

    #[test]
    fn blank_values_are_the_same_as_not_writing_them() {
        assert_eq!(
            parse("[theme]\ndark = \"\"\nlight = \"  \"\n").unwrap(),
            ThemePair::default()
        );
    }

    #[test]
    fn a_path_under_home_is_expanded() {
        assert_eq!(
            parse("[theme]\ndark = \"~/themes/night.tmTheme\"\nlight = \"Solarized (light)\"\n")
                .unwrap(),
            pair(
                Some("/home/u/themes/night.tmTheme"),
                Some("Solarized (light)")
            )
        );
        // `~` 単独や `~user/` は展開しない（`~/` だけ）。HOME が無ければそのまま。
        assert_eq!(
            parse("[theme]\ndark = \"~other/x.tmTheme\"\n").unwrap(),
            pair(Some("~other/x.tmTheme"), None)
        );
        assert_eq!(expand_home("~".into(), Some(Path::new("/home/u"))), "~");
        let section = ThemeSection {
            dark: Some("~/x.tmTheme".into()),
            light: None,
        };
        assert_eq!(section.into_pair(None), pair(Some("~/x.tmTheme"), None));
    }

    #[test]
    fn an_unknown_key_is_an_error_that_names_the_key() {
        // タイプミスを黙って無視しない。
        let err = parse("[theme]\ndrak = \"Dracula\"\n").unwrap_err();
        assert!(err.contains("drak"), "{err}");
    }

    #[test]
    fn a_wrong_type_is_an_error_that_names_the_key() {
        for (text, key) in [
            ("theme = \"Dracula\"\n", "theme"),
            ("[theme]\nlight = [\"a\"]\n", "light"),
            ("[theme]\ndark = 1\n", "dark"),
        ] {
            let err = parse(text).unwrap_err();
            assert!(err.contains(key), "{key}: {err}");
        }
    }

    #[test]
    fn flags_sit_over_the_file_and_theme_covers_both_sides() {
        let file = pair(Some("Catppuccin Mocha"), Some("Catppuccin Latte"));
        let flags = |both: Option<&str>, dark: Option<&str>, light: Option<&str>| ThemeFlags {
            both: both.map(String::from),
            dark: dark.map(String::from),
            light: light.map(String::from),
        };
        // フラグが無ければ設定ファイル、それも無ければ既定（None）。
        assert_eq!(flags(None, None, None).over(Some(&file)), file);
        assert_eq!(flags(None, None, None).over(None), ThemePair::default());
        // 片側のフラグはその側だけを上書きする。
        assert_eq!(
            flags(None, None, Some("Solarized (light)")).over(Some(&file)),
            pair(Some("Catppuccin Mocha"), Some("Solarized (light)"))
        );
        assert_eq!(
            flags(None, Some("Dracula"), None).over(None),
            pair(Some("Dracula"), None),
            "もう片側は既定のまま"
        );
        // `--theme` は両側を上書きし、片側のフラグにも勝つ。
        assert_eq!(
            flags(Some("Dracula"), None, None).over(Some(&file)),
            ThemePair::both("Dracula")
        );
        assert_eq!(
            flags(Some("Dracula"), Some("Nord"), Some("Catppuccin Latte")).over(Some(&file)),
            ThemePair::both("Dracula")
        );
        // 片側だけ書いた設定ファイルは、もう片側を既定に残す。
        assert_eq!(
            flags(None, None, None).over(Some(&pair(None, Some("Catppuccin Latte")))),
            pair(None, Some("Catppuccin Latte"))
        );
    }
}
