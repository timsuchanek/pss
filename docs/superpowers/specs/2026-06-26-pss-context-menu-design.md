# pss — left-sidebar right-click context menu

Date: 2026-06-26
Status: approved (design)

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
  picker into the cascade; refactoring the pre-existing oversize `app.rs`.

## Target row types

The sidebar tree (`App::tree_rows` → `TreeRow.selection`) yields three kinds:

- `Selection::All` — the aggregate root.
- `Selection::Bucket(label)` — a **project**: a group of PIDs. Backed by a
  `Bucket` whose `BucketKey` is one of `Repo(path)`, `Cwd(path)`,
  `Bundle(name)`, `System`, `Unknown`. Menu content varies by subtype.
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
- `j` / `↓` — next item (clamped, no wrap; disabled items are skipped for
  selection movement so the cursor never lands on a dimmed row).
- `k` / `↑` — previous item.
- `Enter` / `l` / `→` — activate: run the action, or open its submenu.
- `h` / `←` — back one level (pop submenu); at root level this is a no-op
  (use Esc to close).
- `Esc` / `q` — pop one submenu level, or close the menu if at root.
- **Mouse:** left-click on an item runs it; left-click outside the menu closes
  it; right-click on another sidebar row re-anchors/rebuilds the menu for that
  row.

### Submenus
- **Renice** opens a nested cascade popup drawn to the right of (or, when it
  would overflow, to the left of / clamped against) the parent item.
- **Kill…** does *not* cascade. It closes the context menu and opens the
  existing centered signal menu (`render_kill_menu`), retargeted to the
  relevant PID set. This reuses a polished, already-built picker. The minor
  style inconsistency (cascade vs centered) is accepted for v1 and noted as a
  possible later unification.

## Menu contents

Icons are indicative; final glyphs chosen during implementation to match the
existing UI palette.

### Process leaf — `Selection::Process(label, pid)`
1. `⊙ Inspect` → `open_drilldown(pid)` + `ensure_drilldown_loaded_for_tab()`.
2. `☠ Kill…` → open signal menu targeting `[pid]`.
3. `‖ Suspend` → SIGSTOP to `pid`.
4. `▸ Resume` → SIGCONT to `pid`.
5. `⚖ Renice ▸` → submenu: `High (-10)`, `Normal (0)`, `Low (+10)`, `Idle (+19)`.
6. `# Copy PID` → clipboard `"<pid>"`.
7. `⌗ Copy command` → clipboard full command line.
8. `📁 Reveal exe in Finder` → `open -R <exe>`.
9. `✎ Open cwd in editor` → editor `<cwd>`.
10. `⌨ Open cwd in terminal` → `open -a <terminal> <cwd>`.

Items 7–10 require the process's exe / cwd. These are read on demand from the
live `sysinfo` snapshot (`process(pid).exe()` / `.cwd()` / `.cmd()`). If a value
is unavailable, the corresponding item is present but **disabled** (rendered
dimmed, not activatable).

Suspend and Resume are both always shown (sending SIGCONT to a running process
or SIGSTOP to a stopped one is harmless), avoiding reliance on possibly-stale
process-status detection.

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
  - `📁 Reveal app in Finder`.
  - The `Kill all N…` item is relabeled to `Quit app (TERM all)` and the menu
    additionally offers `Force-quit (KILL all)`. (Both route through the signal
    targeting machinery.)
- **`System` / `Unknown`:** only `Collapse/Expand` + `Focus`.

### All root — `Selection::All`
1. `Expand all` → clear `app.collapsed`.
2. `Collapse all` → insert every current bucket label into `app.collapsed`.

## Architecture

`app.rs` is already ~1249 lines (over the project's 500-line guideline), so new
logic lives in new, focused modules; `app.rs` gains only small state fields and
thin delegators.

### `src/menu.rs` (new) — pure menu model + logic
```rust
pub enum MenuTarget { All, Bucket(String), Process(String, u32) }

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
    pub target: MenuTarget,
    pub levels: Vec<MenuLevel>,   // stack; last == active
}
```
Responsibilities:
- **Builders:** `build_root(target, subtype, n_pids, has_dir, has_exe, has_cwd)
  -> Vec<MenuItem>` and `build_renice() -> Vec<MenuItem>`. Pure functions of the
  inputs — directly unit-testable.
- **Navigation:** `nav(delta)`, `select() -> SelectOutcome`, `back()`,
  cursor-skip over disabled items.
- The builder takes already-resolved booleans/inputs (subtype, pid count, dir/
  exe/cwd availability) so it has no dependency on `App` and stays pure.

### `src/actions.rs` (new) — side effects, with a testable seam
Command **construction** is separated from **execution**:
```rust
pub enum ShellAction { /* program + args, e.g. Reveal(PathBuf), OpenWith(app, path), Editor(path) */ }
pub fn shell_command(a: &ShellAction, cfg: &ExternalCfg) -> std::process::Command; // testable argv
pub fn run(a: ShellAction, cfg: &ExternalCfg) -> Result<(), String>;

pub fn copy_to_clipboard(text: &str) -> Result<(), String>;   // pbcopy
pub fn renice(pid: u32, niceness: i32) -> Result<(), String>; // libc::setpriority
pub fn signal(pid: u32, sig: sysinfo::Signal);                // suspend/resume reuse
```
- Clipboard: pipe to `pbcopy`.
- Reveal/open: `open <dir>`, `open -R <exe>`, `open -a <terminal> <dir>`,
  editor = configured editor else `$VISUAL` else `$EDITOR` (error string if none).
- Renice: `libc::setpriority(PRIO_PROCESS, pid, niceness)`; on failure
  (negative niceness without privileges) return an error string for the status
  line.

### `src/ui/context_menu.rs` (new) — rendering
- Draws each `MenuLevel` in the stack as a `Clear` + bordered list, cascading.
- **Edge clamping:** if a popup would overflow the bottom, shift it up; if it
  would overflow the right, draw it to the left of its anchor. A pure
  `place(anchor, size, screen) -> Rect` helper is unit-testable.
- Selected item highlighted; disabled items dimmed; `▸` suffix on items that
  open a submenu.

### `src/app.rs` (extend, minimally)
- New fields: `context_menu: Option<ContextMenu>`, `status_msg: Option<(String, Instant)>`.
- Extend `KillMenu`:
  ```rust
  pub struct KillMenu { pub targets: Vec<u32>, pub name: String }
  ```
  `kill_with_signal` loops over `targets`, sends the signal to each, and sets a
  status message (`"sent SIGTERM to N procs"`).
- Delegators: `open_context_menu(target, col, row)`, `context_menu_nav`,
  `context_menu_select`, `context_menu_back`, `close_context_menu`,
  `context_menu_click(col, row)`, and `run_menu_action(action)` which dispatches
  into `actions.rs` / existing methods and sets `status_msg`.
- Helpers: `bucket_pids(label)`, `bucket_dir(label)`, `proc_exe/cwd/cmdline(pid)`,
  `set_status(msg)`.
- `status_msg` is cleared when its `Instant` is older than ~2.5s (checked each
  render or each loop tick).

### `src/main.rs` (wire input)
- `handle_mouse`: add a `Down(MouseButton::Right)` arm → if a sidebar row is hit,
  set selection + `app.open_context_menu(target, col, row)`. While
  `context_menu.is_some()`, intercept `Down(Left)` first: inside-menu → click an
  item; outside → close.
- Key loop: add a context-menu branch in the precedence chain **after** the
  kill-menu branch and **before** the drill-down branch, handling
  `j/k/↑/↓/Enter/l/→/h/←/Esc/q`. Add `KeyCode::Char('x')` in normal mode to open
  the menu on the focused row (compute anchor from geometry).

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
- `renice failed (needs sudo)`
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

## Testing

**Unit (pure):**
- Menu builder produces the right item set + enabled flags per target/subtype:
  - process leaf includes Suspend/Resume/Copy PID; Reveal/Open disabled when
    exe/cwd absent.
  - `Repo`/`Cwd` bucket includes Open-in-editor/terminal + Copy path + Kill-all.
  - `System`/`Unknown` bucket has **no** Kill-all.
  - `Bundle` bucket uses Quit/Force-quit labels + Reveal app.
  - `All` root has only Expand-all/Collapse-all.
- Navigation clamps at ends, skips disabled items, submenu push/pop via
  select/back.
- Renice preset → niceness mapping (`High=-10`, `Normal=0`, `Low=+10`, `Idle=+19`).
- `actions::shell_command` argv construction for reveal/open/editor/terminal
  given an `ExternalCfg` (no process execution in the test).
- `context_menu::place` clamping: anchor near each screen edge flips/shifts the
  popup to stay fully on-screen.

**Manual:**
- Run pss; right-click a `Repo` project, a `Bundle`, `(system)`, and a process
  leaf; confirm the menus differ as specified.
- Exercise Inspect, Kill…, Suspend/Resume, Renice presets, Copy PID/command,
  Reveal, Open-in-editor/terminal, Focus, Expand/Collapse; confirm status-line
  feedback and that disabled items don't activate.
- Verify `x` opens the menu on the focused row and keyboard navigation works.

## Non-goals / future

- Right-click support in the process table and recommendations pane (easy
  extension of the same machinery).
- Mouse-move hover highlight inside the menu.
- Unifying the kill/signal picker into the cascade submenu style.
- A confirmation step for very large `Kill all N` — the deliberate signal pick
  is treated as sufficient for v1.
