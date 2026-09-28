//! 構文の配色（syntect の [`Theme`]）の解決。
//!
//! テーマは two-face の組み込みの名前（`Catppuccin Mocha`・`Solarized (dark)` など）か、
//! `.tmTheme` ファイルのパス（拡張子の大文字小文字は問わない）で指す。ライト用とダーク用の
//! 2 本（[`ThemePair`]）を持ち、背景がライトかで片方を選ぶ。解決できない名前・読めない
//! ファイルは、**その側の既定**（ダーク = [`DEFAULT_DARK`]、ライト = [`DEFAULT_LIGHT`]）へ
//! 落ちる — ライトの背景にダークのテーマの淡い文字色を載せない。
//!
//! akapen・ashiato などの `highlight.rs` の `Highlighter::new` のテーマの部分（と
//! `theme_by_name`・scope の読み替え・既定の文字色）をここへ移したもの。
//!
//! 解決したテーマには Markdown の scope の読み替え（[`apply_markdown_scope_aliases`]）を
//! かけてある。

use std::str::FromStr;
use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use syntect::highlighting::{FontStyle, ScopeSelectors, Theme, ThemeItem, ThemeSet};
use syntect::parsing::Scope;
use two_face::theme::EmbeddedLazyThemeSet;

/// ダーク側の既定（テーマが無いか、解決できないとき）。
pub const DEFAULT_DARK: &str = "Catppuccin Mocha";
/// ライト側の既定。
pub const DEFAULT_LIGHT: &str = "Solarized (light)";

/// 背景に合う既定の名前。
pub fn default_name(light: bool) -> &'static str {
    if light { DEFAULT_LIGHT } else { DEFAULT_DARK }
}

/// ライト用・ダーク用のテーマの対。`None` の側は既定（[`default_name`]）。
///
/// アプリの設定の型（旧 `SyntaxThemes`）と同じ形で、[`crate::config`] が
/// 設定ファイルとフラグから組む。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThemePair {
    /// 背景がダークのとき。
    pub dark: Option<String>,
    /// 背景がライトのとき。
    pub light: Option<String>,
}

impl ThemePair {
    /// 両側に同じテーマ（`--theme <name>` と同じ形）。
    pub fn both(spec: &str) -> Self {
        Self {
            dark: Some(spec.to_string()),
            light: Some(spec.to_string()),
        }
    }

    /// 背景に合う側（`None` は既定）。
    pub fn for_background(&self, light: bool) -> Option<&str> {
        if light {
            self.light.as_deref()
        } else {
            self.dark.as_deref()
        }
    }

    /// 背景に合う側を解決する（[`resolve`]）。
    pub fn resolve(&self, light: bool) -> Theme {
        resolve(self.for_background(light), light)
    }
}

/// テーマの名前かパスを解決する。無い・解決できないときは背景に合う既定へ落ちる。
/// Markdown の scope の読み替えをかけて返す。
pub fn resolve(spec: Option<&str>, light: bool) -> Theme {
    let mut theme = spec.and_then(load).unwrap_or_else(|| {
        by_name(default_name(light)).expect("既定のテーマは two-face に組み込まれている")
    });
    // 新しい文法が出す scope の名前に、古い名前で書かれたテーマの色を写す。
    apply_markdown_scope_aliases(&mut theme);
    theme
}

/// テーマの名前かパスを読む（既定へは落とさない。読み替えもかけない）。`None` は解決できない
/// — 知らない名前か、読めない・壊れた `.tmTheme`。
///
/// `.tmTheme` で終われば（大文字小文字は問わない）ファイルのパス、でなければ two-face の名前。
pub fn load(spec: &str) -> Option<Theme> {
    if is_tmtheme_path(spec) {
        ThemeSet::get_theme(spec).ok()
    } else {
        by_name(spec)
    }
}

/// `.tmTheme` のパスか（拡張子の大文字小文字は問わない）。
pub fn is_tmtheme_path(spec: &str) -> bool {
    const EXT: &[u8] = b".tmtheme";
    let bytes = spec.as_bytes();
    bytes.len() >= EXT.len() && bytes[bytes.len() - EXT.len()..].eq_ignore_ascii_case(EXT)
}

/// two-face の組み込みのテーマを正式な名前（`Catppuccin Mocha`・`Solarized (dark)` など）で
/// 引く。知らない名前は `None`。
pub fn by_name(name: &str) -> Option<Theme> {
    EmbeddedLazyThemeSet::theme_names()
        .iter()
        .copied()
        .find(|t| t.as_name() == name)
        .map(|t| embedded_themes().get(t).clone())
}

/// two-face の組み込みのテーマの名前（`--theme` に渡せるもの）。
pub fn embedded_names() -> impl Iterator<Item = &'static str> {
    EmbeddedLazyThemeSet::theme_names()
        .iter()
        .map(|t| t.as_name())
}

/// two-face の組み込みのテーマ。読み込みは重いので 1 回だけ。
fn embedded_themes() -> &'static EmbeddedLazyThemeSet {
    static THEMES: OnceLock<EmbeddedLazyThemeSet> = OnceLock::new();
    THEMES.get_or_init(two_face::theme::extra)
}

/// Markdown の scope の読み替え。syntect・two-face の Markdown の文法は
/// `markup.raw.code-fence.rust.markdown-gfm` や `markup.heading.1.markdown` を出すが、
/// 外から持ってくる `.tmTheme`（tokyo-night など）の多くは Markdown の色を古い Sublime の
/// 名前（`markup.fenced_code.block.markdown`・`heading.1.markdown`）で書いている。
/// テーマの selector は**前方一致**（`is_prefix_of`）で当たるので、文法が出す名前の頭に
/// 当たる規則がテーマに無ければ、古い名前の規則の色をその頭へ写す — テーマごとに
/// 手を入れずに、作者の意図どおりの色で Markdown が塗られる。コードフェンスの言語の部分
/// （`.rust`）は、その手前までの頭を使って飛ばす。
const MARKDOWN_SCOPE_ALIASES: &[(&str, &str)] = &[
    // （文法が出す頭、テーマがよく書いている古い名前）
    ("markup.raw.code-fence", "markup.fenced_code.block.markdown"),
    ("markup.raw.code-fence", "markup.raw.block.markdown"),
    ("markup.raw.inline", "markup.inline.raw.string.markdown"),
    (
        "meta.code-fence.definition",
        "markup.fenced_code.block.markdown",
    ),
    ("markup.heading.1.markdown", "heading.1.markdown"),
    ("markup.heading.2.markdown", "heading.2.markdown"),
    ("markup.heading.3.markdown", "heading.3.markdown"),
    ("markup.heading.4.markdown", "heading.4.markdown"),
    ("markup.heading.5.markdown", "heading.5.markdown"),
    ("markup.heading.6.markdown", "heading.6.markdown"),
];

/// `sel` のどれかの selector が、`target` と頭を共有する scope に触れているか。
fn scope_selector_mentions(sel: &ScopeSelectors, target: &Scope) -> bool {
    sel.selectors.iter().any(|s| {
        s.path
            .scopes
            .iter()
            .any(|sc| target.is_prefix_of(*sc) || sc.is_prefix_of(*target))
    })
}

/// 古い Markdown の scope の名前の規則を、新しい名前へ写す（読み替えの表はこの関数の上）。
/// syntect の GFM の文法より古いテーマでも、コードフェンスとインラインのコードに色が付く。
/// [`resolve`] はこれをかけて返す。
pub fn apply_markdown_scope_aliases(theme: &mut Theme) {
    for (new_name, legacy_name) in MARKDOWN_SCOPE_ALIASES {
        let Ok(new_scope) = Scope::new(new_name) else {
            continue;
        };
        let Ok(legacy_scope) = Scope::new(legacy_name) else {
            continue;
        };
        // テーマが新しい名前をもう塗っているなら触らない。
        if theme
            .scopes
            .iter()
            .any(|item| scope_selector_mentions(&item.scope, &new_scope))
        {
            continue;
        }
        // 古い名前の規則を、新しい名前で複製する。
        let copies: Vec<ThemeItem> = theme
            .scopes
            .iter()
            .filter(|item| scope_selector_mentions(&item.scope, &legacy_scope))
            .map(|item| ThemeItem {
                scope: ScopeSelectors::from_str(new_name).expect("読み替えの selector"),
                style: item.style,
            })
            .collect();
        theme.scopes.extend(copies);
    }
}

/// テーマが文字色を持たないときの文字色。
const DEFAULT_FG_DARK: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
const DEFAULT_FG_LIGHT: Color = Color::Rgb(0x30, 0x30, 0x40);

/// テーマが文字色を持たないときの、背景に合う文字色。
pub fn default_fg_fallback(light: bool) -> Color {
    if light {
        DEFAULT_FG_LIGHT
    } else {
        DEFAULT_FG_DARK
    }
}

/// テーマの文字色（地の文の色）。テーマに無ければ [`default_fg_fallback`]。
pub fn default_fg(theme: &Theme, light: bool) -> Color {
    theme
        .settings
        .foreground
        .map_or(default_fg_fallback(light), to_color)
}

/// syntect の色を ratatui の色にする（アルファは捨てる）。
pub fn to_color(c: syntect::highlighting::Color) -> Color {
    Color::Rgb(c.r, c.g, c.b)
}

/// scope 1 つ（`markup.heading.2.markdown` など）を syntect と同じ規則でテーマに当てる:
/// いちばん当たる規則が勝ち、規則は具体的なものほど後から効く。テーマに `scope` に触れる
/// 規則が無ければ `None`。
pub fn scope_style(theme: &Theme, scope: &str) -> Option<Style> {
    let scope = Scope::new(scope).ok()?;
    let highlighter = syntect::highlighting::Highlighter::new(theme);
    let m = highlighter.style_mod_for_stack(&[scope]);
    if m.foreground.is_none() && m.background.is_none() && m.font_style.is_none() {
        return None;
    }
    let mut style = Style::default();
    if let Some(fg) = m.foreground {
        style = style.fg(to_color(fg));
    }
    if let Some(bg) = m.background {
        style = style.bg(to_color(bg));
    }
    if let Some(fs) = m.font_style {
        if fs.contains(FontStyle::BOLD) {
            style = style.add_modifier(Modifier::BOLD);
        }
        if fs.contains(FontStyle::ITALIC) {
            style = style.add_modifier(Modifier::ITALIC);
        }
        if fs.contains(FontStyle::UNDERLINE) {
            style = style.add_modifier(Modifier::UNDERLINED);
        }
    }
    Some(style)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 文字色が真っ赤なだけの最小の .tmTheme（plist の XML）。
    const MINIMAL_TM_THEME: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>name</key>
  <string>Minimal</string>
  <key>settings</key>
  <array>
    <dict>
      <key>settings</key>
      <dict>
        <key>foreground</key>
        <string>#ff0000</string>
      </dict>
    </dict>
  </array>
</dict>
</plist>
"#;

    /// Markdown のコードフェンスを古い名前（`markup.fenced_code.block.markdown`）でしか
    /// 書いていないテーマ（syntect の GFM の文法はもうこの名前を出さない）。
    const LEGACY_TM_THEME: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>name</key>
  <string>Legacy</string>
  <key>settings</key>
  <array>
    <dict>
      <key>settings</key>
      <dict>
        <key>foreground</key>
        <string>#a9b1d6</string>
      </dict>
    </dict>
    <dict>
      <key>scope</key>
      <string>markup.fenced_code.block.markdown</string>
      <key>settings</key>
      <dict>
        <key>foreground</key>
        <string>#ff0000</string>
      </dict>
    </dict>
  </array>
</dict>
</plist>
"#;

    fn write_theme(dir: &tempfile::TempDir, name: &str, text: &str) -> String {
        let path = dir.path().join(name);
        std::fs::write(&path, text).unwrap();
        path.to_str().unwrap().to_string()
    }

    fn background(theme: &Theme) -> Option<syntect::highlighting::Color> {
        theme.settings.background
    }

    #[test]
    fn the_defaults_are_embedded_and_differ_by_background() {
        let dark = by_name(DEFAULT_DARK).unwrap();
        let light = by_name(DEFAULT_LIGHT).unwrap();
        assert_ne!(background(&dark), background(&light));
        assert_eq!(default_name(false), DEFAULT_DARK);
        assert_eq!(default_name(true), DEFAULT_LIGHT);
        assert!(embedded_names().any(|n| n == DEFAULT_DARK));
        assert!(embedded_names().any(|n| n == DEFAULT_LIGHT));
    }

    #[test]
    fn absent_or_unresolvable_themes_fall_back_to_the_default_for_the_background() {
        // ライトの背景は決してダークのテーマの淡い文字色に落ちない（逆も）。
        let dark = background(&by_name(DEFAULT_DARK).unwrap());
        let light = background(&by_name(DEFAULT_LIGHT).unwrap());
        for spec in [
            None,
            Some("no-such-theme"),
            Some("tokyo-night"),
            Some("/no/such.tmTheme"),
            Some(""),
        ] {
            assert_eq!(background(&resolve(spec, false)), dark, "{spec:?}");
            assert_eq!(background(&resolve(spec, true)), light, "{spec:?}");
        }
    }

    #[test]
    fn a_name_resolves_on_either_background() {
        let nord = by_name("Nord").unwrap();
        assert_eq!(background(&resolve(Some("Nord"), true)), background(&nord));
        assert_eq!(background(&resolve(Some("Nord"), false)), background(&nord));
        // 名前は正式な綴りそのもの（大文字小文字も）。
        assert!(load("nord").is_none());
    }

    #[test]
    fn a_tmtheme_path_loads_the_file_whatever_the_case_of_the_extension() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["minimal.tmTheme", "minimal2.tmtheme", "minimal3.TMTHEME"] {
            let path = write_theme(&dir, name, MINIMAL_TM_THEME);
            let theme = resolve(Some(&path), false);
            assert_eq!(
                default_fg(&theme, false),
                Color::Rgb(0xff, 0x00, 0x00),
                "{name}"
            );
        }
        assert!(is_tmtheme_path("x.TmTheme"));
        assert!(!is_tmtheme_path("Catppuccin Mocha"));
        assert!(!is_tmtheme_path("tmtheme"));
        // 多バイト文字で終わっても落ちない。
        assert!(!is_tmtheme_path("テーマ"));
    }

    #[test]
    fn a_broken_tmtheme_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_theme(&dir, "broken.tmTheme", "<plist>");
        assert!(load(&path).is_none());
        assert_eq!(
            background(&resolve(Some(&path), true)),
            background(&by_name(DEFAULT_LIGHT).unwrap())
        );
    }

    #[test]
    fn a_pair_picks_the_side_for_the_background() {
        let pair = ThemePair {
            dark: Some("Nord".into()),
            light: None,
        };
        assert_eq!(pair.for_background(false), Some("Nord"));
        assert_eq!(pair.for_background(true), None);
        assert_eq!(
            background(&pair.resolve(false)),
            background(&by_name("Nord").unwrap())
        );
        assert_eq!(
            background(&pair.resolve(true)),
            background(&by_name(DEFAULT_LIGHT).unwrap())
        );
        assert_eq!(
            ThemePair::both("Nord"),
            ThemePair {
                dark: Some("Nord".into()),
                light: Some("Nord".into())
            }
        );
        assert_eq!(ThemePair::default().for_background(true), None);
    }

    #[test]
    fn legacy_markdown_code_scope_is_aliased() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_theme(&dir, "legacy.tmTheme", LEGACY_TM_THEME);
        let theme = resolve(Some(&path), false);
        // 文法が出す新しい名前に、古い名前の色（#ff0000）が当たる。
        let style = scope_style(&theme, "markup.raw.code-fence.markdown-gfm").unwrap();
        assert_eq!(style.fg, Some(Color::Rgb(0xff, 0x00, 0x00)));
        // 読み替えをかけない load では当たらない。
        let raw = load(&path).unwrap();
        assert_eq!(
            scope_style(&raw, "markup.raw.code-fence.markdown-gfm"),
            None
        );
    }

    #[test]
    fn aliases_leave_themes_that_already_style_the_new_scope_alone() {
        let mut theme = by_name(DEFAULT_DARK).unwrap();
        let before = theme.scopes.len();
        apply_markdown_scope_aliases(&mut theme);
        let after = theme.scopes.len();
        apply_markdown_scope_aliases(&mut theme);
        // 2 度かけても増えない（1 度目で新しい名前が塗られる）。
        assert_eq!(theme.scopes.len(), after);
        assert!(after >= before);
    }

    #[test]
    fn the_default_foreground_comes_from_the_theme_else_the_fallback() {
        let mocha = by_name(DEFAULT_DARK).unwrap();
        let fg = mocha.settings.foreground.unwrap();
        assert_eq!(default_fg(&mocha, false), Color::Rgb(fg.r, fg.g, fg.b));
        let mut bare = mocha.clone();
        bare.settings.foreground = None;
        assert_eq!(default_fg(&bare, false), Color::Rgb(0xcd, 0xd6, 0xf4));
        assert_eq!(default_fg(&bare, true), Color::Rgb(0x30, 0x30, 0x40));
    }

    #[test]
    fn scope_style_resolves_like_syntect() {
        let theme = resolve(None, false);
        assert!(scope_style(&theme, "markup.heading.2.markdown").is_some_and(|s| s.fg.is_some()));
        assert_eq!(scope_style(&theme, "no.such.scope.anywhere"), None);
        assert_eq!(scope_style(&theme, ""), None);
    }
}
