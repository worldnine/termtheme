# termtheme

Light/dark plumbing for terminal apps built on ratatui + syntect.

日本語版 README は [README.ja.md](README.ja.md) にあります。

- **`background`** — ask the terminal for its background color at startup (OSC 11) and decide light or dark (ITU-R BT.601 luma). 150 ms timeout, tolerant of keys typed while waiting.
- **`scheme`** — mode 2031: subscribe to color-scheme change notifications (`CSI ? 2031 h` / `l`), query the current scheme (`CSI ? 996 n`). Sent as crossterm `Command`s, or managed by `Subscription`, which unsubscribes on drop and while a child process owns the terminal.
- **`input`** — read terminal input *instead of* crossterm's `event::poll` / `event::read`. You get the same `crossterm::event::Event`s, plus `Input::ColorScheme` (mode 2031 notifications) and the color answers `Input::Background` (OSC 11), `Input::Foreground` (OSC 10) and `Input::Palette` (OSC 4).
- **`colors`** — query the foreground (OSC 10) and palette (OSC 4) colors too, and wait for the answers without dropping the keys typed meanwhile (`input::wait_for_colors`).
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

`Input` is `#[non_exhaustive]`: more kinds of terminal reports may come, so keep a catch-all arm like `other =>` above.

## Subscribing without leaking

A mode 2031 subscription is terminal state: it outlives the process. Leave it on and the next program on the terminal — the shell, or a TUI you open with Enter — receives the notifications, and anything reading with crossterm stalls there. `scheme::Subscription` does the bookkeeping:

```rust
use termtheme::scheme::Subscription;

let mut scheme = Subscription::new(); // stdout if it's a terminal, else /dev/tty
if !fixed {                           // --light / --dark: never subscribe
    scheme.start()?;                  // after the startup OSC 11 detection
}

// Handing the terminal to a child ($EDITOR, Enter-to-open), or stopping on Ctrl+Z:
scheme.suspend()?;                    // CSI ? 2031 l
// ... run the child / raise(SIGTSTP) ...
scheme.resume()?;                     // CSI ? 2031 h, then CSI ? 996 n for what changed meanwhile

// Dropping it unsubscribes — early `?` returns and panic unwinds included.
```

Nothing is written unless `start` was called, nothing between `suspend` and `resume` (the child owns the terminal, and may have subscribed itself), and repeated calls are no-ops. `Subscription::fixed()` never writes or opens anything. `Subscription::stdout()` suits apps drawing through ratatui's `CrosstermBackend<Stdout>` (all `Stdout` handles share one buffer, so the order is kept).

Signal handlers don't run `Drop`. An app that dies on SIGINT / SIGTERM calls `scheme::unsubscribe_in_signal_handler()` from its handler: one atomic load and one `write(2)` to where the subscription is live, nothing while suspended. `scheme::DISABLE_UPDATES` is the raw sequence for handlers that build their own restore bytes.

## Asking for the palette

`background` answers light or dark. Apps that blend the terminal's actual colors also need the foreground and the 16 ANSI colors. Send a `colors::QueryColors` and wait for the answers on the input reader — keys typed while waiting are kept and come out of the next `input::poll` / `input::read`, in order:

```rust
use std::time::Duration;
use crossterm::execute;
use termtheme::{colors::QueryColors, input};

let query = QueryColors::ansi(); // foreground, background, palette 0-15
execute!(std::io::stdout(), &query)?;
let colors = input::wait_for_colors(&query, Duration::from_millis(200))?;
if let (Some(fg), Some(bg), Some(ansi)) = (colors.foreground, colors.background, colors.ansi()) {
    // draw with the terminal's own colors
}
```

It returns what arrived by the deadline (terminals that don't answer give `None`s); answers later than that arrive as `Input::Foreground` / `Input::Background` / `Input::Palette`. `QueryForeground` and `QueryPalette(n)` send single queries.

## Probe

```sh
cargo run --example probe                # read with termtheme
cargo run --example probe -- --crossterm # read with crossterm, for comparison
```

Prints the startup detection, subscribes to mode 2031 and prints every notification with a timestamp. `s` / `b` re-query the scheme / background, `p` queries the foreground, background and 16 colors and waits for them, `z` stops (unsubscribed until `fg`). `q` or Ctrl+C quits (and unsubscribes; so do SIGTERM and SIGHUP).

## Third-party code

`src/input/parse.rs` is derived from [crossterm](https://github.com/crossterm-rs/crossterm) 0.29.0 (`src/event/sys/unix/parse.rs`), MIT License, Copyright (c) 2019 Timon. The notice is kept in the file header.

## License

MIT
