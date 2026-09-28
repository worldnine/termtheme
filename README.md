# termtheme

Light/dark plumbing for terminal apps built on ratatui + syntect.

日本語版 README は [README.ja.md](README.ja.md) にあります。

- **`background`** — ask the terminal for its background color at startup (OSC 11) and decide light or dark (ITU-R BT.601 luma). 150 ms timeout, tolerant of keys typed while waiting.
- **`scheme`** — mode 2031: subscribe to color-scheme change notifications (`CSI ? 2031 h` / `l`), query the current scheme (`CSI ? 996 n`). Sent as crossterm `Command`s.
- **`input`** — read terminal input *instead of* crossterm's `event::poll` / `event::read`. You get the same `crossterm::event::Event`s, plus `Input::ColorScheme` (mode 2031 notifications) and `Input::Background` (OSC 11 answers).
- **`theme`** — resolve a theme name (two-face's embedded themes) or a `.tmTheme` path into a `syntect::highlighting::Theme`; a `{ dark, light }` pair picked by background; per-side defaults (`Catppuccin Mocha` / `Solarized (light)`); markdown scope aliases for older themes.
- **`config`** — a serde type for a `[theme]` table (`dark` / `light`, unknown keys rejected), blank-means-unset, `~/` expansion, `$XDG_CONFIG_HOME/<app>` resolution, and `--theme` / `--theme-dark` / `--theme-light` layering. All pure: the environment and home directory are passed in.

Dependencies follow the apps that use it: crossterm 0.29, ratatui 0.30, syntect 5.3, two-face 0.5.

## Why an input reader

crossterm 0.29 cannot receive mode 2031 notifications. Its parser only ends a `CSI ?` sequence on `u` or `c`, so after `CSI ? 997 ; 1 n` it keeps buffering **every following byte** until one of those arrives — and because the tty is blocking, `event::poll(timeout)` does not return in the meantime: the whole event loop stalls. Late OSC 11 answers turn into `Alt+]`, `1`, `1`, `;`, `r`, … key events. Both are reproduced on a real pty in [`tests/crossterm_2031.rs`](tests/crossterm_2031.rs).

`termtheme::input` carries a copy of crossterm's input parser with those two cases fixed, so keys parse exactly as before:

```rust
use std::time::Duration;
use termtheme::input::{self, Input};

loop {
    if input::poll(Duration::from_millis(100))? {
        match input::read()? {
            Input::Event(event) => { /* your existing crossterm Event arms */ }
            other => {
                if let Some(light) = other.light() {
                    // rebuild the theme and redraw
                }
            }
        }
    }
}
```

Don't mix it with crossterm's `event::poll` / `event::read` (two readers would fight over the tty), and avoid ratatui's `Terminal::clear()` in fullscreen apps: it calls crossterm's `cursor::position()`, which runs crossterm's reader.

## Probe

```sh
cargo run --example probe                # read with termtheme
cargo run --example probe -- --crossterm # read with crossterm, for comparison
```

Prints the startup detection, subscribes to mode 2031 and prints every notification with a timestamp. `q` or Ctrl+C quits (and unsubscribes).

## Third-party code

`src/input/parse.rs` is derived from [crossterm](https://github.com/crossterm-rs/crossterm) 0.29.0 (`src/event/sys/unix/parse.rs`), MIT License, Copyright (c) 2019 Timon. The notice is kept in the file header.

## License

MIT
