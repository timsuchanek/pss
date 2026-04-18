# pss

A system monitor that groups processes by the directory they're working in,
instead of dumping a flat `ps` list. Built with [ratatui].

Think `htop`, but organized around the projects you're actually building.

```
┌─ pss ── CPU ██░░░░░░░░ 12%   MEM ██████░░░░ 58% (37/64 GB)   procs 982   recs ai ──┐
├─ projects ──────────────────┬─ cpu over time — ~/code/pss   peak 214% ──────────────┤
│ ▣ all                 216%  │ 200┤          ▄▆▇██▆▅▄                                │
│ ▼ ~/code/pss    🦀    198%  │ 100┤      ▂▄▆█████████████▇▅▃                          │
│     cargo [9981]      184%  │   0┼────────────────────────────────────────────►      │
│     rustc [9983]       14%  │  █ cargo 184%   ▓ rustc 14%   ░ node 2%   · other      │
│ ▸ ~/code/orbit         18%  ├─────────────────────────────────────────────────────────┤
│ ▸ (system)              8%  │ mem over time — ~/code/pss   peak 4.2G                 │
│ ▸ (chrome bundle)      62%  │ 4.2G┤                     ▄▆▇█▇▆                       │
│                             │ 2.1G┤         ▂▄▆████████████████▇▅                    │
├─ processes — sort cpu ──────┴─────────────────────────────────────────────────────────┤
│ pid    cpu%   last 60s     mem    name    cmd                                         │
│ 9981  184.2   ▂▃▅▆▇███     4.2G   cargo   cargo build --release                       │
│ ...                                                                                   │
├─ recommendations — ai ────────────────────────────────────────────────────────────────┤
│ ☠  vite [11402]              conf  96%  ~1800M   dev server idle ~47m, holding 1800M   │
│ ◆  chrome · 23 helpers       conf  55%  ~4100M   lots of tabs; consider closing some   │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

## Features

- **Projects, not processes.** The sidebar groups procs by their `cwd`. If the
  cwd lives inside a git repo, the repo root becomes the bucket. The tree
  expands to show the child processes inline.
- **Two stacked time-series charts.** CPU on top, memory underneath, sharing a
  stable color map per pid so you can trace "this one thing is my cpu *and*
  mem hog".
- **Recommendations that actually work offline.** Local heuristics surface
  idle dev servers (`vite`, `webpack`, `esbuild`, …), orphaned CLI tools,
  large idle mem holders, and chrome-helper sprawl.
- **Smarter recs with OpenRouter (optional).** Set `OPENROUTER_API_KEY` and
  a small JSON digest is sent every ~10s; the LLM returns structured
  `{action, target, reason, confidence}` rows that take over for 45s. If it
  goes silent, the local heuristics seamlessly reclaim the panel.
- **Kill from anywhere.** `Shift+K` sends `SIGTERM` to either the highlighted
  process or the highlighted recommendation.
- **Live-resize every pane.** Drag the dividers with the mouse: sidebar
  width, chart height, recs height.
- **Cross-platform day one.** macOS, Linux, Windows — `sysinfo` + `crossterm`
  + `ratatui` + `gix`, pure Rust, no system libs.

## Install

```sh
cargo install --git https://github.com/timsuchanek/pss --root ~/.local
```

Or from a local checkout:

```sh
git clone https://github.com/timsuchanek/pss
cd pss
cargo install --path . --root ~/.local --force
```

Make sure `~/.local/bin` is on your `PATH`. Then:

```sh
pss
```

## Config

Everything is optional. Without any config, pss runs offline with local
recommendations.

### OpenRouter (smarter recommendations)

Either export an env var:

```sh
export OPENROUTER_API_KEY=sk-or-...
```

…or drop a config file:

- macOS: `~/Library/Application Support/pss/config.toml`
- Linux: `~/.config/pss/config.toml`
- Windows: `%APPDATA%\pss\config.toml`

```toml
openrouter_api_key = "sk-or-..."
model = "anthropic/claude-haiku-4.5"
```

Any OpenRouter-served model works; pick one that's fast and cheap since it
fires every 10 seconds.

## Keybindings

```
j / k / ↑ ↓       navigate within the current pane
h / l / ← →       collapse / expand the tree, or move to the right pane
tab               cycle panes
enter             (processes pane) lock selection
Shift+K           SIGTERM the highlighted process or recommendation
c / m / n         sort processes by cpu / mem / name
?                 help overlay
q  /  esc  /  ^C  quit
mouse drag        resize panes at their shared borders
```

## How it groups processes

1. If the process's `cwd` is inside a git repo → bucket is the repo root.
2. Else if `cwd` is a real directory → bucket is that literal path.
3. Else if the process lives inside an `.app` bundle or `Program Files` →
   bucket is the app bundle.
4. `PID 1`, `launchd`, and anything with no `cwd` → `(system)`.

This means you see **"my build is eating 200% in ~/code/pss"** instead of
"cargo and 8 rustc workers at various percentages".

## Stack

- [`ratatui`] + [`crossterm`] — TUI & input
- [`sysinfo`] — processes, cpu%, mem, cwd, kill
- [`gix`] — repo-root discovery, pure Rust
- [`tokio`] + [`reqwest`] — async LLM calls, rustls only, no OpenSSL
- Custom half-block stacked-area chart widget (~200 LOC)

[ratatui]: https://ratatui.rs
[`ratatui`]: https://ratatui.rs
[`crossterm`]: https://github.com/crossterm-rs/crossterm
[`sysinfo`]: https://github.com/GuillaumeGomez/sysinfo
[`gix`]: https://github.com/Byron/gitoxide
[`tokio`]: https://tokio.rs
[`reqwest`]: https://github.com/seanmonstar/reqwest

## Status

Early. Works on macOS day-to-day; Linux and Windows should build and run but
are less tested. Please open an issue if you see something weird.

## License

MIT
