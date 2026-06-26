# pss — left-sidebar right-click context menu

Date: 2026-06-26
Status: approved (design, rev 2 — incorporates spec review)

## Why

The left sidebar is read-only today. You can sort, collapse, drill in — but to
act on a process (kill, suspend, inspect, find its folder) you reach for
separate keystrokes or leave the tool. A right-click context menu turns the
sidebar into something actionable: point at a project or a process and do the
obvious thing. It also surfaces capabilities pss already has (kill signals,
drill-down, collapse) plus a few cheap, high-value additions (suspend/resume,
clipboard, reveal/open, renice) behind one discoverable gesture.

## Scope

- **In scope (v1):** right-click context menu on the **left sidebar** rows only —
  the project buckets (overview) and the process leaves (sub-points). Keyboard
  trigger `x` on the focused row. Cascade submenu for renice. Transient status
  feedback. Config for editor/terminal apps.
- **Out of scope (v1, YAGNI):** right-click on the process table or
  recommendations pane; mouse-move hover highlight; unifying the kill/signal
  picker into the cascade; refactoring the pre-existing large `app.rs`; full
  Linux/Windows command support for the external actions (see Platform support).

## Platform support

The repo already gates platform-specific behavior with `cfg` (`src/details.rs`,
`src/thermal.rs`, `src/netmon.rs` are per-OS; `src/app.rs:1234` wraps signal
sending in `#[cfg(unix)]`/`#[cfg(not(unix))]`). The menu follows the same
convention rather than hard-coding macOS everywhere.

| Action group              | macOS (v1)                 | Linux                      | Windows        |
|---------------------------|----------------------------|----------------------------|----------------|
| Kill / signals            | existing `#[cfg(unix)]`     | existing `#[cfg(unix)]`     | item disabled  |
| Suspend / Resume          | SIGSTOP/SIGCONT (`cfg(unix)`) | same                     | item disabled  |
| Renice                    | `libc::setpriority` (`cfg(unix)`) | same                  | item disabled  |
| Copy PID / command / path | `pbcopy`                   | disabled in v1             | disabled in v1 |
| Reveal in Finder          | `open` / `open -R`         | disabled in v1             | disabled in v1 |
| Open in terminal          | `open -a <terminal>`       | disabled in v1             | disabled in v1 |
| Open in editor            | `$VISUAL`/`$EDITOR`        | `$VISUAL`/`$EDITOR`        | `$VISUAL`/`$EDITOR` |

`Command::new("pbcopy")` etc. compile on every platform, so non-macOS builds do
**not** break — but to avoid dead, silently-failing items, every external action
exposes `supported() -> bool` (false on platforms it can't serve). The menu
builder **disables** (dims) unsupported items so they never activate. Linux /
Windows equivalents (`xdg-open`/`wl-copy`/`xclip`, `clip`/`explorer`/`start`) are
a documented future, not v1.

**Verification expectation:** v1 is developed and verified on macOS. The cfg/
`supported()` seam guarantees a clean compile elsewhere; the non-macOS items are
disabled. True Linux/Windows command behavior is out of scope for v1.

## Target row types

The sidebar tree (`App::tree_rows` → `TreeRow.selection`) yields three kinds,
modeled by the existing `Selection` enum (`src/app.rs:128`), which the menu
**reuses as its target** (no parallel `MenuTarget` enum — avoids drift):

- `Selection::All` — the aggregate root.
- `Selection::Bucket(label)` — a **project**: a group of PIDs. Backed by a
  `Bucket` whose `BucketKey` is one of `Repo(path)`, `Cwd(path)`,
  `Bundle(name)`, `System`, `Unknown`. Menu content varies by subtype, resolved
  via `bucket_key(label)`.
- `Selection::Process(label, pid)` — a single process (leaf).

## Behavior

### Opening
- **Mouse:** `MouseEventKind::Down(MouseButton::Right)` on a sidebar row. Sets
  the selection to that row, then opens the menu anchored at the click
  `(column, row)`.
- **Keyboard:** `x` in normal mode opens the menu on the currently focused
  sidebar row. The anchor `(col, row)` is computed in `main.rs` (which knows
  terminal geometry, as it already does for mouse hit-testing) from the focused
  tree-row index and `sidebar_list_top`.

### Navigating an open menu
- `j` / `↓` — next item (clamped, no wrap; disabled items are skipped so the
  cursor never lands on a dimmed row).
- `k` / `↑` — previous item.
- `Enter` / `l` / `→` — activate: run the action, or open its submenu.
- `h` / `←` — back one level (pop submenu); at root level this is a no-op
  (use Esc to close).
- `Esc` / `q` — pop one submenu level, or close the menu if at root.
- **Mouse:** left-click on an item runs it; left-click outside the menu closes
  it; right-click on another sidebar row re-anchors/rebuilds the menu for that
  row.

### Close-on-activate (required for correct input routing)
Activating a **terminal action** (anything that is not "open a submenu") closes
the context menu *before* the action runs. This is not cosmetic: the context
menu sits **above** the drill-down modal in the input-precedence chain (see
Input precedence), so if `Inspect` left the menu open, the menu branch would
swallow the modal's `j/k/Esc/…` keys and the drill-down would be input-blocked.

Concretely, `select()` on a menu level returns a `SelectOutcome`:
- `Close` — terminal actions (Inspect, Suspend, Resume, Copy*, Reveal*, Open*,
  Renice(value), Focus, ToggleCollapse, Expand/CollapseAll). The dispatcher runs
  the side effect and clears `context_menu`. `Inspect` then opens the drill-down.
- `OpenSubmenu` — `Renice ▸` pushes a cascade level; menu stays open.
- `OpenKillPicker(targets, name)` — `Kill…` clears `context_menu` and opens the
  existing centered signal menu.

### Submenus
- **Renice** opens a nested cascade popup drawn to the right of (or, when it
  would overflow, to the left of / clamped against) the parent item.
- **Kill…** does *not* cascade. It closes the context menu and opens the
  existing centered signal menu (`render_kill_menu`), retargeted to the
  relevant PID set. This reuses a polished, already-built picker. The minor
  style inconsistency (cascade vs centered) is accepted for v1 and noted as a
  possible later unification.

### Stale targets
Menu item enabled/disabled state is evaluated at build time. If a process exits
while the menu is open, an activated action resolves its target at activation
time; a now-invalid PID surfaces as a status-line error via the action's
`Result<(), String>` error path. v1 does **not** re-validate items on each render.

## Menu contents

Icons are indicative; final glyphs chosen during implementation to match the
existing UI palette.

### Process leaf — `Selection::Process(label, pid)`
1. `⊙ Inspect` → close menu, `open_drilldown(pid)` + `ensure_drilldown_loaded_for_tab()`.
2. `☠ Kill…` → open signal menu targeting `[pid]`.
3. `‖ Suspend` → SIGSTOP to `pid`.
4. `▸ Resume` → SIGCONT to `pid`.
5. `⚖ Renice ▸` → submenu: `High (-10)`, `Normal (0)`, `Low (+10)`, `Idle (+19)`.
6. `# Copy PID` → clipboard `"<pid>"`.
7. `⌗ Copy command` → clipboard the cached command line, falling back to the
   process name when `cmd` is empty (always enabled; `name` is always present).
8. `📁 Reveal exe in Finder` → `open -R <exe>` (macOS).
9. `✎ Open cwd in editor` → editor `<cwd>`.
10. `⌨ Open cwd in terminal` → `open -a <terminal> <cwd>` (macOS).

Process metadata (exe / cwd / cmd / status) is read from the **cached
`ProcSample`** via `app.process_by_pid(pid)` (the collector caches all of these
each sample — `src/collector.rs:69-90`). No fresh `sysinfo::System` is created
on menu open. Items 8–9 (and 9–10 reveal/open) are **disabled** (dimmed) when
the needed exe/cwd is absent, or when the action is unsupported on the platform.

Suspend and Resume are both always shown (sending SIGCONT to a running process
or SIGSTOP to a stopped one is harmless), avoiding reliance on possibly-stale
status detection. (`ProcSample.status` is available if we later want to show
only the relevant one.)

### Project bucket — `Selection::Bucket(label)`
Common items (all subtypes):
1. `▾ Collapse` / `▸ Expand` (toggles `app.collapsed`).
2. `☠ Kill all N…` → open signal menu targeting **all** PIDs in the bucket,
   where `N = bucket.pids.len()`. **Omitted for `System` and `Unknown`** — no
   mass-signalling of system processes.
3. `⊙ Focus` → collapse every *other* bucket so only this one is expanded.

Subtype-specific additions:
- **`Repo(path)` / `Cwd(path)`** (have a folder):
  - `📁 Reveal in Finder` → `open <path>`.
  - `✎ Open in editor` → editor `<path>`.
  - `⌨ Open in terminal` → `open -a <terminal> <path>`.
  - `# Copy path` → clipboard `<path>`.
- **`Bundle(name)`** (an app):
  - `📁 Reveal app in Finder` → reveal the `.app` directory. The path is derived
    on demand by walking a member process's cached exe up to its nearest `.app`
    ancestor — the same logic `bucket_for` already uses to detect bundles
    (`src/app.rs:852`). Disabled when no `.app` ancestor is recoverable.
  - Kill is the generic `☠ Kill all N…` picker (no separate Quit/Force-quit
    items — dropped as YAGNI; the signal picker already offers TERM and KILL).
- **`System` / `Unknown`:** only `Collapse/Expand` + `Focus`.

### All root — `Selection::All`
1. `Expand all` → clear `app.collapsed`.
2. `Collapse all` → insert every current bucket label into `app.collapsed`.

## Architecture

`app.rs` is already ~1249 lines, so new logic lives in new, focused modules to
keep `app.rs` from growing further; `app.rs` gains only small state fields and
thin delegators.

### `src/menu.rs` (new) — pure menu model + logic
```rust
use crate::app::Selection;

pub enum MenuAction {
    Inspect,
    OpenKill,                 // -> centered signal menu, target = this row's PIDs
    Suspend, Resume,
    OpenReniceSubmenu,
    Renice(i32),              // preset niceness
    CopyPid, CopyCommand, CopyPath,
    RevealExe, RevealDir, OpenEditor, OpenTerminal,
    ToggleCollapse, Focus,
    ExpandAll, CollapseAll,
}

pub struct MenuItem {
    pub icon: &'static str,
    pub label: String,
    pub action: MenuAction,
    pub enabled: bool,
    pub opens_submenu: bool,
}

pub struct MenuLevel {
    pub title: Option<String>,
    pub items: Vec<MenuItem>,
    pub selected: usize,
    pub origin_col: u16,      // top-left anchor for this level
    pub origin_row: u16,
}

pub struct ContextMenu {
    pub target: Selection,        // reuses the app's Selection enum
    pub levels: Vec<MenuLevel>,   // stack; last == active
}

pub enum SelectOutcome { Close, OpenSubmenu, OpenKillPicker(Vec<u32>, String) }
```
Responsibilities:
- **Builders** (pure functions of already-resolved inputs — directly testable):
  - `build_process(has_exe, has_cwd) -> Vec<MenuItem>`
  - `build_bucket(subtype, n_pids, has_dir, has_app_path) -> Vec<MenuItem>`
  - `build_all() -> Vec<MenuItem>`
  - `build_renice() -> Vec<MenuItem>`
  Each external item's `enabled` also factors `actions::supported(_)`.
- **Navigation:** `nav(delta)` (skips disabled items), `select() -> SelectOutcome`,
  `back()`.
- The builders take resolved booleans/inputs (subtype, pid count, dir/exe/cwd/
  app-path availability) so `menu.rs` never touches `App` internals and stays
  pure. The only `app` dependency is the plain-data `Selection` enum.

### `src/actions.rs` (new) — side effects, with a testable seam
Command **construction** is separated from **execution**:
```rust
pub struct ExternalCfg { pub editor: Option<String>, pub terminal: String }

pub enum ShellAction { RevealDir(PathBuf), RevealFile(PathBuf), Editor(PathBuf), Terminal(PathBuf) }
pub fn supported(a: &MenuAction) -> bool;                          // platform gate
pub fn shell_command(a: &ShellAction, cfg: &ExternalCfg) -> Option<std::process::Command>; // testable argv
pub fn run(a: ShellAction, cfg: &ExternalCfg) -> Result<(), String>;

pub fn copy_to_clipboard(text: &str) -> Result<(), String>;       // pbcopy (macOS)
#[cfg(unix)] pub fn renice(pid: u32, niceness: i32) -> Result<(), String>; // libc::setpriority
pub fn signal(pid: u32, sig: sysinfo::Signal);                    // suspend/resume, reuse cfg(unix) path
```
- Clipboard: pipe to `pbcopy`; `supported()` false off macOS.
- Reveal/open: `open <dir>`, `open -R <exe>`, `open -a <terminal> <dir>`,
  editor = configured editor else `$VISUAL` else `$EDITOR` (error string if none).
- Renice: `libc::setpriority(PRIO_PROCESS, pid, niceness)`; on failure
  (negative niceness without privileges) return an error string for the status
  line.
- Tests construct argv via `shell_command` and assert with `Command::get_program`
  / `get_args` — no process is executed.

### `src/ui/context_menu.rs` (new) — rendering
- Draws each `MenuLevel` in the stack as a `Clear` + bordered list, cascading.
- **Edge clamping:** a pure `place(anchor, size, screen) -> Rect` helper — if a
  popup would overflow the bottom, shift it up; if it would overflow the right,
  draw it to the left of its anchor.
- Selected item highlighted; disabled items dimmed; `▸` suffix on items that
  open a submenu.

### `src/app.rs` (extend, minimally)
- New fields: `context_menu: Option<ContextMenu>`, `status_msg: Option<(String, Instant)>`.
- **`KillMenu` shape change.** Current (app.rs:200):
  ```rust
  pub struct KillMenu { pub pid: u32, pub name: String }
  ```
  New:
  ```rust
  pub struct KillMenu { pub targets: Vec<u32>, pub name: String }
  ```
  `name` stays; `pid` is replaced by `targets`. Call sites to update:
  - `open_kill_menu` ctor (app.rs:468) → `KillMenu { targets: vec![pid], name }`.
  - add `open_kill_menu_targets(targets: Vec<u32>, name: String)` for bucket/menu use.
  - `kill_with_signal` (app.rs:476) loops over `targets`, signalling each, then
    sets `status_msg` (`"sent SIGTERM to N procs"`).
  - `render_kill_menu` (ui/mod.rs:159) title: `kill [pid] name` for a single
    target, `kill N procs · <name>` for multiple (`name` = proc name when single,
    bucket label when multiple).
  - The `K` key openers (main.rs:335, 368) call `open_kill_menu()` and are
    unaffected.
- Delegators: `open_context_menu(selection, col, row)`, `context_menu_nav`,
  `context_menu_select`, `context_menu_back`, `close_context_menu`,
  `context_menu_click(col, row)`, and `run_menu_action(action)` which dispatches
  into `actions.rs` / existing methods and sets `status_msg`.
- Helpers: `bucket_key(label) -> Option<BucketKey>`, `bucket_pids(label) -> Vec<u32>`,
  `bucket_dir(label) -> Option<PathBuf>`, `bucket_app_path(label) -> Option<PathBuf>`,
  and exe/cwd/cmd via the existing `process_by_pid(pid)`. `set_status(msg)`.
- `status_msg` is cleared when its `Instant` is older than ~2.5s (checked each
  render or each loop tick).

### `src/main.rs` (wire input)
- `handle_mouse`: add a `Down(MouseButton::Right)` arm → if a sidebar row is hit,
  set selection + `app.open_context_menu(selection, col, row)`. While
  `context_menu.is_some()`, intercept `Down(Left)` first: inside-menu → click an
  item; outside → close.
- Key loop: add a context-menu branch in the precedence chain **after** the
  kill-menu branch and **before** the drill-down branch, handling
  `j/k/↑/↓/Enter/l/→/h/←/Esc/q`. Add `KeyCode::Char('x')` in normal mode to open
  the menu on the focused row (compute anchor from geometry). `x` is currently
  unbound in normal mode (confirmed).

### `src/config.rs` (extend)
```toml
[external]
editor = ""          # empty => $VISUAL then $EDITOR
terminal = "Terminal"
```
Sensible defaults; both optional.

## Feedback (status line)

A transient `status_msg` (default lifetime ~2.5s) is rendered in the footer area
(`render_footer`), replacing or augmenting the hint line while active, in a
highlighted style. The static footer hint gains `x menu`. Example messages:
- `copied pid 13545`
- `copied command`
- `sent SIGSTOP to 13545`
- `sent SIGKILL to 7 procs`
- `reniced 13545 → +10`
- `renice failed (not permitted)`
- `no $EDITOR set`
- `revealed in Finder`

## Input precedence (final order in `main.rs` key loop)

1. Ctrl-C → quit
2. search-mode
3. thermal overlay
4. kill menu
5. **context menu** (new)
6. drill-down modal
7. normal-mode keys (adds `x`)

The Close-on-activate rule (above) is what makes #5-above-#6 safe: `Inspect`
closes the context menu before opening the drill-down, so the modal receives its
own keys.

## Testing

Rust inline test modules (`#[cfg(test)] mod tests { … }`) — these are the
project's first unit tests, so the convention is established here: tests live in
the same file as the code under test (`menu.rs`, `actions.rs`,
`ui/context_menu.rs`).

**Unit (pure):**
- Menu builder produces the right item set + enabled flags per target/subtype:
  - process leaf includes Suspend/Resume/Copy PID/Copy command; Reveal/Open
    disabled when exe/cwd absent **or** unsupported on platform.
  - Copy command falls back to name when cmd is empty (still enabled).
  - `Repo`/`Cwd` bucket includes Open-in-editor/terminal + Copy path + Kill-all.
  - `System`/`Unknown` bucket has **no** Kill-all.
  - `Bundle` bucket includes Reveal-app (disabled when no app-path) and the
    generic Kill-all (no Quit/Force-quit items).
  - `All` root has only Expand-all/Collapse-all.
- Navigation clamps at ends, skips disabled items; submenu push/pop via
  select/back; `select()` returns `Close` for terminal actions, `OpenSubmenu`
  for Renice, `OpenKillPicker` for Kill….
- Renice preset → niceness mapping (`High=-10`, `Normal=0`, `Low=+10`, `Idle=+19`).
- `actions::supported` gates clipboard/reveal/terminal off non-macOS.
- `actions::shell_command` argv construction for reveal/open/editor/terminal
  given an `ExternalCfg` (no process execution in the test).
- `context_menu::place` clamping: anchor near each screen edge flips/shifts the
  popup to stay fully on-screen.

**Manual:**
- Run pss; right-click a `Repo` project, a `Bundle`, `(system)`, and a process
  leaf; confirm the menus differ as specified.
- Exercise Inspect (confirm the drill-down then receives keys), Kill…,
  Suspend/Resume, Renice presets, Copy PID/command, Reveal, Open-in-editor/
  terminal, Focus, Expand/Collapse; confirm status-line feedback and that
  disabled items don't activate.
- Verify `x` opens the menu on the focused row and keyboard navigation works.

## Non-goals / future

- Right-click support in the process table and recommendations pane (easy
  extension of the same machinery).
- Mouse-move hover highlight inside the menu.
- Unifying the kill/signal picker into the cascade submenu style.
- A confirmation step for very large `Kill all N` — the deliberate signal pick
  is treated as sufficient for v1.
- Linux/Windows command implementations for clipboard/reveal/terminal (the
  `supported()` seam is in place to add them later).
