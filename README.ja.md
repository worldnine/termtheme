# termtheme

ratatui + syntect で作る端末アプリのための、ライト／ダークの配管。

- **`background`** — 起動時に端末へ背景色を問い合わせ（OSC 11）、明るさ（ITU-R BT.601 の輝度）でライトかダークかを決める。150 ms で諦める。待っているあいだに打たれたキーに強い
- **`scheme`** — モード 2031: 配色が変わったときの知らせの購読（`CSI ? 2031 h` / `l`）と、今の配色の問い合わせ（`CSI ? 996 n`）。crossterm の `Command` として送る
- **`input`** — crossterm の `event::poll` / `event::read` の**代わりに**端末の入力を読む。crossterm と同じ `crossterm::event::Event` に加えて、`Input::ColorScheme`（モード 2031 の知らせ）と `Input::Background`（OSC 11 の答え）を渡す
- **`theme`** — テーマの名前（two-face の組み込み）か `.tmTheme` のパスを `syntect::highlighting::Theme` にする。背景で選ぶ `{ dark, light }` の対、側ごとの既定（`Catppuccin Mocha` / `Solarized (light)`）、古いテーマのための Markdown の scope の読み替え
- **`config`** — 設定ファイルの `[theme]` の表の serde の型（`dark` / `light`、知らないキーは断る）、空は無いのと同じ、`~/` の展開、`$XDG_CONFIG_HOME/<アプリ名>` の解決、`--theme` / `--theme-dark` / `--theme-light` との重ね方。どれも実環境を読まない（環境変数とホームは渡す）

依存の版は、使うアプリにそろえてある: crossterm 0.29、ratatui 0.30、syntect 5.3、two-face 0.5。

## なぜ入力の読み手を持つか

crossterm 0.29 はモード 2031 の知らせを受けられない。crossterm の読み手は `CSI ?` で始まる列を `u` か `c` でしか閉じないので、`CSI ? 997 ; 1 n` の後ろの**入力を全部**そのどちらかが来るまで溜め込む。しかも端末の fd はブロッキングなので、そのあいだ `event::poll(timeout)` が戻らない — イベントループごと止まる。遅れて届いた OSC 11 の答えは `Alt+]`・`1`・`1`・`;`・`r`… のキーに化ける。どちらも実物の pty で再現してある（[`tests/crossterm_2031.rs`](tests/crossterm_2031.rs)）。

`termtheme::input` は crossterm の読み手を写し、この 2 つだけを直したもの。キーの読み方は今までと同じ:

```rust
use std::time::Duration;
use termtheme::input::{self, Input};

loop {
    if input::poll(Duration::from_millis(100))? {
        match input::read()? {
            Input::Event(event) => { /* 今までの crossterm の Event の腕 */ }
            other => {
                if let Some(light) = other.light() {
                    // テーマを作り直して描き直す
                }
            }
        }
    }
}
```

crossterm の `event::poll` / `event::read` と混ぜない（読み手が 2 つになると端末を取り合う）。全画面のアプリでは ratatui の `Terminal::clear()` も避ける（crossterm の `cursor::position()` を呼び、crossterm の読み手が動く）。

## 確かめる道具

```sh
cargo run --example probe                # termtheme で読む
cargo run --example probe -- --crossterm # crossterm で読む（比べる用）
```

起動時の判定の結果を出し、モード 2031 を購読して、知らせが届くたびに時刻と値を出す。`q` か Ctrl+C で終わる（購読を外す）。

## ほかの人のコード

`src/input/parse.rs` は [crossterm](https://github.com/crossterm-rs/crossterm) 0.29.0 の `src/event/sys/unix/parse.rs` を元にしている（MIT License、Copyright (c) 2019 Timon）。表記はファイルの冒頭に残してある。

## ライセンス

MIT
