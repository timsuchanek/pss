# pss

A system monitor that groups processes by the directory they're working in,
instead of dumping a flat `ps` list. Built with [ratatui].

Think `htop`, but organized around the projects you're actually building.

```
┌─ pss ── CPU ██░░░░░░░░ 12%   MEM ██████░░░░ 58% (37/64 GB)   therm cpu 62.4° gpu 48.1°   ↑1.2M ↓340K   procs 982   recs ai ──┐
├─ projects ─────────────────────────────────┬─ cpu over time — ~/code/pss   peak 214% ──────────────────────────────────────────┤
│                       cpu    mem  ↑    ↓   │ 200┤          ▄▆▇██▆▅▄                                                            │
│ ▣ all                216%  37.0G   1M  340K│ 100┤      ▂▄▆█████████████▇▅▃                                                      │
│ ▼ ~/code/pss    🦀   198%   4.6G  12K   2K │   0┼────────────────────────────────────────────►                                  │
│     cargo [9981]     184%   4.2G   0    0  │  █ cargo 184%   ▓ rustc 14%   ░ node 2%   · other                                  │
│     rustc [9983]      14%   420M  12K   2K ├─────────────────────────────────────────────────────────────────────────────────────┤
│ ▸ ~/code/orbit        18%   1.1G   8K   3K │ mem over time — ~/code/pss   peak 4.2G                                              │
│ ▸ (system)             8%   2.4G   0    0  │ 4.2G┤                     ▄▆▇█▇▆                                                    │
│ ▸ (chrome bundle)     62%  12.8G 800K 280K │ 2.1G┤         ▂▄▆████████████████▇▅                                                 │
├─ processes — sort cpu ─────────────────────┴─────────────────────────────────────────────────────────────────────────────────────┤
│ pid    cpu%   last 60s     mem      ↑       ↓     name    cmd                                                                    │
│ 9981  184.2   ▂▃▅▆▇███     4.2G     0       0     cargo   cargo build --release                                                  │
│ ...                                                                                                                              │
├─ recommendations — ai ───────────────────────────────────────────────────────────────────────────────────────────────────────────┤
│ ☠  vite [11402]              conf  96%  ~1800M   dev server idle ~47m, holding 1800M                                              │
│ ◆  chrome · 23 helpers       conf  55%  ~4100M   lots of tabs; consider closing some                                              │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

## Features

- **Projects, not processes.** The sidebar groups procs by their `cwd`. If the
  cwd lives inside a git repo, the repo root becomes the bucket. The tree
  expands to show the child processes inline.
- **Per-bucket cpu / mem / net columns.** Sortable by clicking the header
  (or `c` / `m` / `w` from the keyboard). Net is split into ↑ upload and ↓
  download so a noisy talker stands out from a noisy listener. Sort
  direction toggles on repeat clicks.
- **Two stacked time-series charts.** CPU on top, memory underneath, sharing a
  stable color map per pid so you can trace "this one thing is my cpu *and*
  mem hog".
- **Per-PID network rates.** macOS spawns a streaming `nettop -d` child and
  attributes B/s to each process. The header shows aggregate rates (read
  via `getifaddrs` — no sudo, no shell-out).
- **Native thermal sensors (macOS).** Reads labelled IOHID temperature
  sensors directly (same path Stats.app / asitop use). Header shows max
  CPU / GPU °C; press `T` for the full sensor list overlay.
- **Drill-down modal.** `Enter` on any process pops a 5-tab inspector:
  facts (full cmd, cwd, uid, parent, threads, fds), env, open files
  (`lsof`), sockets (lsof TCP/UDP with peers), and the process tree under
  the selected PID.
- **Fuzzy filter.** `/` opens a [nucleo]-powered live filter that matches on
  process name, force-expands matching buckets, and shows live match
  counts in the sidebar title.
- **Kill signal menu.** `Shift+K` opens a signal picker — TERM, KILL, HUP,
  INT, STOP, CONT, QUIT, USR1, USR2 — that targets the highlighted
  process or recommendation.
- **htop-parity controls.** `space` to pause sampling, `[` / `]` to step the
  sampling interval (250ms granularity), `H` to hide kernel threads, `U`
  to show only your uid, `S` to hide pss itself.
- **Recommendations that work offline.** Local heuristics surface idle dev
  servers (`vite`, `webpack`, `esbuild`, …), orphaned CLI tools, large
  idle mem holders, and chrome-helper sprawl.
- **Smarter recs with OpenRouter (optional).** Set `OPENROUTER_API_KEY` and
  a small JSON digest is sent every ~10s; the LLM returns structured
  `{action, target, reason, confidence}` rows that take over for 45s. If it
  goes silent, the local heuristics seamlessly reclaim the panel.
- **Click-to-select everywhere.** Mouse selects rows in any pane. Drag the
  dividers to live-resize the sidebar, the chart, or the recs pane.
- **State persists between runs.** Sidebar/chart/recs sizes, sort key &
  direction, sampling interval, filter toggles, and which buckets are
  collapsed all save to `config.toml`.
- **Cross-platform core.** macOS, Linux, Windows for the base monitor —
  `sysinfo` + `crossterm` + `ratatui` + `gix`, pure Rust, no system libs.
  Per-PID network and thermal sensors are macOS-only today.

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

UI state (pane sizes, sort, filters, sampling interval, collapsed buckets)
is written back to the config file on exit, so the next launch comes up
exactly how you left it.

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
navigation
  j / k / ↑ ↓       move within the current pane
  h / l / ← →       collapse / expand tree, or move to right pane
  tab               cycle panes
  enter             open drill-down modal on the selected process
  /                 fuzzy filter (nucleo); esc cancels, enter commits

sorting
  c / m / n / w     sort by cpu / mem / name / network
                    (in the sidebar these toggle direction on repeat)

actions
  Shift+K           kill — opens a signal menu (TERM/KILL/HUP/INT/STOP/…)
  x  / right-click   context menu for the focused project / process / suggestion
                     (inspect · kill · suspend · renice · copy · reveal/open)
  space             pause / resume sampling
  [  /  ]           sampling interval -250ms / +250ms

filters
  H                 hide kernel threads
  U                 show only my uid
  S                 hide pss itself

overlays
  T                 thermal sensor list (macOS)
  ?                 help overlay

global
  esc               clear filter / close overlay / quit
  q  /  ^C          quit
  mouse drag        resize panes at their shared borders
  click             select row · right-click for context menu · header to sort
```

### Drill-down modal (after `enter`)

```
1 / 2 / 3 / 4 / 5   facts · env · files · sockets · tree
h / l · tab         prev / next tab
j / k · PgUp/PgDn   scroll within tab
r                   refresh the active tab's data
K                   open the kill-signal menu for this PID
esc                 close modal
```

### Kill signal menu (after `Shift+K`)

```
enter / t   SIGTERM       k   SIGKILL      h   SIGHUP
i           SIGINT        s   SIGSTOP      c   SIGCONT
q           SIGQUIT       1   SIGUSR1      2   SIGUSR2
esc         cancel
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
- [`tui-tree-widget`] — the projects tree
- [`gix`] — repo-root discovery, pure Rust
- [`nucleo-matcher`] — fuzzy filtering
- [`tokio`] + [`reqwest`] — async LLM calls, rustls only, no OpenSSL
- [`directories`] + [`toml`] — config & persisted UI state
- macOS: `IOHIDEventSystemClient` (via `core-foundation`) for thermal
  sensors; `getifaddrs` for aggregate net; a streaming `nettop -d` child
  for per-PID rx/tx
- Custom half-block stacked-area chart widget (~300 LOC)

[ratatui]: https://ratatui.rs
[`ratatui`]: https://ratatui.rs
[`crossterm`]: https://github.com/crossterm-rs/crossterm
[`sysinfo`]: https://github.com/GuillaumeGomez/sysinfo
[`tui-tree-widget`]: https://github.com/EdJoPaTo/tui-rs-tree-widget
[`gix`]: https://github.com/Byron/gitoxide
[`nucleo-matcher`]: https://github.com/helix-editor/nucleo
[`tokio`]: https://tokio.rs
[`reqwest`]: https://github.com/seanmonstar/reqwest
[`directories`]: https://github.com/dirs-dev/directories-rs
[`toml`]: https://github.com/toml-rs/toml
[nucleo]: https://github.com/helix-editor/nucleo

## Status

Early. Works on macOS day-to-day; Linux and Windows build and run for the
core monitor but per-PID network and thermal sensors are macOS-only.
Please open an issue if you see something weird.

## License

MIT
