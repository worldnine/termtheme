# termtheme

ratatui + syntect で作る端末アプリのための、ライト／ダークの配管。

- **`background`** — 起動時に端末へ背景色を問い合わせ（OSC 11）、明るさ（ITU-R BT.601 の輝度）でライトかダークかを決める。150 ms で諦める。待っているあいだに打たれたキーに強い
- **`scheme`** — モード 2031: 配色が変わったときの知らせの購読（`CSI ? 2031 h` / `l`）と、今の配色の問い合わせ（`CSI ? 996 n`）。crossterm の `Command` として送るか、張り外しを `Subscription` に任せる（落とせば外す、子プロセスに端末を渡すあいだは外す）
- **`input`** — crossterm の `event::poll` / `event::read` の**代わりに**端末の入力を読む。crossterm と同じ `crossterm::event::Event` に加えて、`Input::ColorScheme`（モード 2031 の知らせ）と色の答え — `Input::Background`（OSC 11）・`Input::Foreground`（OSC 10）・`Input::Palette`（OSC 4）— を渡す
- **`colors`** — 文字色（OSC 10）とパレット（OSC 4）も問い合わせ、待つあいだに打たれたキーを捨てずに答えを待つ（`input::wait_for_colors`）
- **`theme`** — テーマの名前（two-face の組み込み）か `.tmTheme` のパスを `syntect::highlighting::Theme` にする。背景で選ぶ `{ dark, light }` の対、側ごとの既定（`Catppuccin Mocha` / `Solarized (light)`）、古いテーマのための Markdown の scope の読み替え
- **`config`** — 設定ファイルの `[theme]` の表の serde の型（`dark` / `light`、知らないキーは断る）、空は無いのと同じ（`non_blank`。`[theme]` の外のキーにも使える）、`~/` の展開、`$XDG_CONFIG_HOME/<アプリ名>` の解決、`--theme` / `--theme-dark` / `--theme-light` との重ね方。どれも実環境を読まない（環境変数とホームは渡す）

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

`Input` は `#[non_exhaustive]`: 端末の報告の種類はこの先も増えうるので、上の `other =>` のような受け皿を置く。

## 購読の張り外し

モード 2031 の購読は端末の状態で、プロセスが終わっても残る。外さずに終わると、次に端末を使うもの — シェル、Enter で開いた別の TUI — に知らせが届き、crossterm で読むものはそこで止まる。張り外しは `scheme::Subscription` に任せる:

```rust
use termtheme::scheme::Subscription;

let mut scheme = Subscription::new(); // stdout が端末ならそこ、でなければ /dev/tty
if !fixed {                           // --light / --dark で固定なら張らない
    scheme.start()?;                  // 起動時の判定（OSC 11）の後で
}

// 子プロセス（$EDITOR、Enter で開くもの）に端末を渡すとき、Ctrl+Z で止まるとき:
scheme.suspend()?;                    // CSI ? 2031 l
// …子を走らせる / raise(SIGTSTP)…
scheme.resume()?;                     // CSI ? 2031 h、続けて CSI ? 996 n（離れていたあいだの切り替えを拾う）

// 落とせば外す — `?` での早い戻りも、panic の巻き戻しも。
```

`start` を呼んでいなければ何も書かず、`suspend` から `resume` までも書かず（端末は子のもの。子が自分で張った購読を外さない）、同じ呼び出しを重ねても 1 回と同じ。`Subscription::fixed()` は何も書かず、何も開かない。ratatui の `CrosstermBackend<Stdout>` で描くアプリは `Subscription::stdout()` でよい（`Stdout` の handle はどれも 1 つのバッファを共有するので、順が崩れない）。

シグナルハンドラの中では `Drop` が走らない。SIGINT・SIGTERM で終わるアプリは、ハンドラから `scheme::unsubscribe_in_signal_handler()` を呼ぶ（原子的な読み 1 つと、張っている書き先への `write(2)` 1 回だけ。`suspend` 中は何もしない）。戻しの列を自分で組むハンドラには、列そのもの `scheme::DISABLE_UPDATES` もある。

## 色をまとめて問い合わせる

ライトかダークかは `background` で足りる。端末の実際の色を混ぜて描くアプリは、文字色と 16 色も要る。`colors::QueryColors` を送り、入力の読み手で答えを待つ — 待つあいだに打たれたキーは取っておかれ、次の `input::poll` / `input::read` から届いた順に出る:

```rust
use std::time::Duration;
use crossterm::execute;
use termtheme::{colors::QueryColors, input};

let query = QueryColors::ansi(); // 文字色・背景色・パレットの 0〜15 番
execute!(std::io::stdout(), &query)?;
let colors = input::wait_for_colors(&query, Duration::from_millis(200))?;
if let (Some(fg), Some(bg), Some(ansi)) = (colors.foreground, colors.background, colors.ansi()) {
    // 端末の色で描く
}
```

締め切りまでに届いた分を返す（答えない端末では `None`）。締め切りに遅れた答えは `Input::Foreground` / `Input::Background` / `Input::Palette` として届く。1 つずつ送るなら `QueryForeground`・`QueryPalette(n)`。

## 確かめる道具

```sh
cargo run --example probe                # termtheme で読む
cargo run --example probe -- --crossterm # crossterm で読む（比べる用）
```

起動時の判定の結果を出し、モード 2031 を購読して、知らせが届くたびに時刻と値を出す。`s` / `b` で配色・背景色を問い合わせ直し、`p` で文字色・背景色・16 色を問い合わせて答えを待ち、`z` で止まる（`fg` まで購読を外す）。`q` か Ctrl+C で終わる（購読を外す。SIGTERM・SIGHUP でも）。

## ほかの人のコード

`src/input/parse.rs` は [crossterm](https://github.com/crossterm-rs/crossterm) 0.29.0 の `src/event/sys/unix/parse.rs` を元にしている（MIT License、Copyright (c) 2019 Timon）。表記はファイルの冒頭に残してある。

## ライセンス

MIT
