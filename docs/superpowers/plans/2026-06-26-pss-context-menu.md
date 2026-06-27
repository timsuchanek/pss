# pss Left-Sidebar Context Menu Implementation Plan (rev 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a right-click (and `x`-key) context menu to the left sidebar that acts on the hovered project bucket or process — inspect, kill/suspend/renice, copy, reveal/open — via an anchored popup with a cascade renice submenu.

**Architecture:** A pure `menu.rs` (model + builders + navigation) describes the menu; `actions.rs` performs side effects behind a testable command-construction seam; `ui/context_menu.rs` holds pure geometry (`place`/`submenu_origin`/`menu_hit`) plus rendering; `menu_dispatch.rs` runs actions over `&mut App` (keeps `app.rs` from ballooning); `app.rs` holds the `ContextMenu` state + thin delegators; `main.rs` wires mouse/keyboard. The existing kill/signal picker is reused, retargeted from a single PID to a PID set.

**Tech Stack:** Rust, ratatui + crossterm (TUI), sysinfo (process control + signals), libc (renice), toml/serde (config). Target platform: macOS (external shell-outs gated; non-macOS items disabled).

## Global Constraints

- **Platform gating:** `actions::caps()` returns a `Caps { clipboard, finder, terminal, signals, renice }`. macOS → all true. Other Unix (Linux) → `signals`/`renice` true, GUI shell-outs (clipboard/finder/terminal) false. Non-Unix → all false. Builders disable items whose capability is false. Non-macOS/Linux builds must still compile.
- **Reuse the cache:** Process exe/cwd/cmd come from the cached `ProcSample` via `App::process_by_pid(pid)` (method at `src/app.rs:619`; the fields are populated by the collector at `src/collector.rs:69-90`). Do **not** create a fresh `sysinfo::System` to read metadata.
- **TUI safety:** pss holds raw mode + alternate screen + mouse capture (`src/main.rs:128-130`) for its whole lifetime. **Never** spawn a TTY program (e.g. `vim`) into the live terminal. "Open in editor" launches a GUI app via macOS `open` (`open -a <app>` or `open -t <path>`).
- **File size:** Keep new files focused; do not pile logic onto the already-large `src/app.rs` (~1249 lines). Action dispatch lives in `src/menu_dispatch.rs`; only state fields + thin delegators go in `app.rs`.
- **Close-on-activate:** Activating any terminal (non-submenu) action clears `context_menu` before running, so the drill-down modal (lower in input precedence) receives its own keys.
- **Conventional Commits** for every commit.
- **Selection reuse:** The menu's target is the existing `crate::app::Selection` enum — no parallel target enum.

## File Structure

| File | Responsibility | Change |
|------|----------------|--------|
| `src/netmon.rs` | (pre-flight) fix existing clippy lint so the final gate is meaningful | Modify |
| `src/menu.rs` | Pure menu model, builders, navigation, action→outcome mapping | Create |
| `src/actions.rs` | Side effects: clipboard, reveal/open, renice, signal; testable argv | Create |
| `src/ui/context_menu.rs` | Pure geometry (`place`/`submenu_origin`/`menu_hit`) + rendering | Create |
| `src/config.rs` | `[external]` editor/terminal config | Modify |
| `src/menu_dispatch.rs` | Action dispatch + menu open/click over `&mut App` | Create |
| `src/app.rs` | `KillMenu` retarget, `ContextMenu` state, status line, thin delegators | Modify |
| `src/ui/mod.rs` | Render context menu, multi-PID kill title, footer status + hint, term_size | Modify |
| `src/main.rs` | Module decls, right-click + `x` open, key/mouse routing, status expiry | Modify |

---

### Task 1: Pre-flight — fix the pre-existing clippy lint

The final gate (`cargo clippy --all-targets -- -D warnings`) currently fails on a pre-existing lint unrelated to this feature. Fix it first so the gate is meaningful.

**Files:**
- Modify: `src/netmon.rs:157`

- [ ] **Step 1: Confirm the failure**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: FAIL with `lines_filter_map_ok` (or similar) at `src/netmon.rs:157` (`reader.lines().flatten()`).

- [ ] **Step 2: Fix the lint**

In `src/netmon.rs`, change line 157:

```rust
                for line in reader.lines().map_while(Result::ok) {
```

- [ ] **Step 3: Confirm clippy is clean**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings.

- [ ] **Step 4: Commit**

```bash
git add src/netmon.rs
git commit -m "fix(netmon): silence lines_filter_map_ok clippy lint"
```

---

### Task 2: `menu.rs` — pure menu model, builders, navigation

**Files:**
- Create: `src/menu.rs`
- Modify: `src/main.rs` (add `mod menu;`)

**Interfaces:**
- Consumes: `crate::app::Selection` (existing enum: `All`, `Bucket(String)`, `Process(String, u32)`).
- Produces:
  - `Caps { clipboard, finder, terminal, signals, renice: bool }`
  - `BucketKind { RepoOrCwd, Bundle, SystemOrUnknown }`
  - `MenuAction` (enum, `#[derive(Clone, PartialEq, Eq, Debug)]`)
  - `MenuItem { icon: &'static str, label: String, action: MenuAction, enabled: bool, opens_submenu: bool }`
  - `MenuLevel { items, selected, origin_col, origin_row }` with `new`, `nav`
  - `ContextMenu { target: Selection, levels: Vec<MenuLevel> }` with `new`, `nav`, `selected_item`, `push`, `pop`
  - `Outcome { Close, Submenu, KillPicker }` + `outcome_for(&MenuAction) -> Outcome`
  - `build_process(caps, has_exe, has_cwd) -> Vec<MenuItem>`
  - `build_bucket(caps, kind, n_pids, has_dir, has_app_path, collapsed) -> Vec<MenuItem>`
  - `build_all() -> Vec<MenuItem>`
  - `build_renice() -> Vec<MenuItem>`

- [ ] **Step 1: Register the module**

In `src/main.rs`, add `mod menu;` to the module list (keep alphabetical-ish, after `mod llm;`):

```rust
mod llm;
mod menu;
mod netmon;
```

- [ ] **Step 2: Write `src/menu.rs`**

```rust
//! Pure model for the left-sidebar context menu: item builders, level
//! navigation, and the action→outcome mapping. No side effects and no `App`
//! access beyond the plain-data `Selection` enum — everything here is unit
//! testable.

use crate::app::Selection;

/// Platform capabilities for the actions. Produced by `actions::caps()`; passed
/// into builders so unsupported items render disabled.
#[derive(Clone, Copy, Debug)]
pub struct Caps {
    pub clipboard: bool,
    pub finder: bool,
    pub terminal: bool,
    pub signals: bool,
    pub renice: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BucketKind {
    RepoOrCwd,
    Bundle,
    SystemOrUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuAction {
    Inspect,
    OpenKill,
    Suspend,
    Resume,
    OpenReniceSubmenu,
    Renice(i32),
    CopyPid,
    CopyCommand,
    CopyPath,
    RevealExe,
    RevealDir,
    OpenEditor,
    OpenTerminal,
    ToggleCollapse,
    Focus,
    ExpandAll,
    CollapseAll,
}

#[derive(Clone, Debug)]
pub struct MenuItem {
    pub icon: &'static str,
    pub label: String,
    pub action: MenuAction,
    pub enabled: bool,
    pub opens_submenu: bool,
}

#[derive(Clone, Debug)]
pub struct MenuLevel {
    pub items: Vec<MenuItem>,
    pub selected: usize,
    pub origin_col: u16,
    pub origin_row: u16,
}

impl MenuLevel {
    pub fn new(items: Vec<MenuItem>, origin_col: u16, origin_row: u16) -> Self {
        let selected = items.iter().position(|i| i.enabled).unwrap_or(0);
        Self { items, selected, origin_col, origin_row }
    }

    /// Move the cursor by `delta`, clamping at the ends and skipping disabled
    /// items. Stays put if there is no enabled item in that direction.
    pub fn nav(&mut self, delta: i32) {
        if self.items.is_empty() {
            return;
        }
        let n = self.items.len() as i32;
        let step = if delta >= 0 { 1 } else { -1 };
        let mut i = self.selected as i32 + step;
        while i >= 0 && i < n {
            if self.items[i as usize].enabled {
                self.selected = i as usize;
                return;
            }
            i += step;
        }
    }
}

#[derive(Clone, Debug)]
pub struct ContextMenu {
    pub target: Selection,
    pub levels: Vec<MenuLevel>,
}

impl ContextMenu {
    pub fn new(target: Selection, items: Vec<MenuItem>, col: u16, row: u16) -> Self {
        Self { target, levels: vec![MenuLevel::new(items, col, row)] }
    }

    pub fn nav(&mut self, delta: i32) {
        if let Some(level) = self.levels.last_mut() {
            level.nav(delta);
        }
    }

    pub fn selected_item(&self) -> Option<&MenuItem> {
        let level = self.levels.last()?;
        level.items.get(level.selected)
    }

    pub fn push(&mut self, items: Vec<MenuItem>, col: u16, row: u16) {
        self.levels.push(MenuLevel::new(items, col, row));
    }

    /// Pop a submenu level. Returns false when already at the root (nothing
    /// popped) so the caller can decide whether to close the whole menu.
    pub fn pop(&mut self) -> bool {
        if self.levels.len() > 1 {
            self.levels.pop();
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Close,
    Submenu,
    KillPicker,
}

/// Maps an action to what activating it does to the menu. Terminal actions
/// close the menu (so lower-precedence overlays get their keys); the two
/// submenu-ish actions are the exceptions.
pub fn outcome_for(action: &MenuAction) -> Outcome {
    match action {
        MenuAction::OpenReniceSubmenu => Outcome::Submenu,
        MenuAction::OpenKill => Outcome::KillPicker,
        _ => Outcome::Close,
    }
}

fn item(
    icon: &'static str,
    label: &str,
    action: MenuAction,
    enabled: bool,
    opens_submenu: bool,
) -> MenuItem {
    MenuItem { icon, label: label.to_string(), action, enabled, opens_submenu }
}

pub fn build_process(caps: Caps, has_exe: bool, has_cwd: bool) -> Vec<MenuItem> {
    vec![
        item("⊙", "Inspect", MenuAction::Inspect, true, false),
        item("☠", "Kill…", MenuAction::OpenKill, caps.signals, false),
        item("‖", "Suspend", MenuAction::Suspend, caps.signals, false),
        item("▸", "Resume", MenuAction::Resume, caps.signals, false),
        item("⚖", "Renice", MenuAction::OpenReniceSubmenu, caps.renice, true),
        item("#", "Copy PID", MenuAction::CopyPid, caps.clipboard, false),
        item("⌗", "Copy command", MenuAction::CopyCommand, caps.clipboard, false),
        item("📁", "Reveal exe in Finder", MenuAction::RevealExe, caps.finder && has_exe, false),
        item("✎", "Open cwd in editor", MenuAction::OpenEditor, caps.finder && has_cwd, false),
        item("⌨", "Open cwd in terminal", MenuAction::OpenTerminal, caps.terminal && has_cwd, false),
    ]
}

pub fn build_bucket(
    caps: Caps,
    kind: BucketKind,
    n_pids: usize,
    has_dir: bool,
    has_app_path: bool,
    collapsed: bool,
) -> Vec<MenuItem> {
    let mut v = Vec::new();
    if collapsed {
        v.push(item("▸", "Expand", MenuAction::ToggleCollapse, true, false));
    } else {
        v.push(item("▾", "Collapse", MenuAction::ToggleCollapse, true, false));
    }
    if kind != BucketKind::SystemOrUnknown {
        let label = format!("Kill all {} …", n_pids);
        v.push(item("☠", &label, MenuAction::OpenKill, caps.signals && n_pids > 0, false));
    }
    v.push(item("⊙", "Focus", MenuAction::Focus, true, false));
    match kind {
        BucketKind::RepoOrCwd => {
            v.push(item("📁", "Reveal in Finder", MenuAction::RevealDir, caps.finder && has_dir, false));
            v.push(item("✎", "Open in editor", MenuAction::OpenEditor, caps.finder && has_dir, false));
            v.push(item("⌨", "Open in terminal", MenuAction::OpenTerminal, caps.terminal && has_dir, false));
            v.push(item("#", "Copy path", MenuAction::CopyPath, caps.clipboard && has_dir, false));
        }
        BucketKind::Bundle => {
            v.push(item("📁", "Reveal app in Finder", MenuAction::RevealDir, caps.finder && has_app_path, false));
        }
        BucketKind::SystemOrUnknown => {}
    }
    v
}

pub fn build_all() -> Vec<MenuItem> {
    vec![
        item("▸", "Expand all", MenuAction::ExpandAll, true, false),
        item("▾", "Collapse all", MenuAction::CollapseAll, true, false),
    ]
}

pub fn build_renice() -> Vec<MenuItem> {
    vec![
        item("△", "High (-10)", MenuAction::Renice(-10), true, false),
        item("─", "Normal (0)", MenuAction::Renice(0), true, false),
        item("▽", "Low (+10)", MenuAction::Renice(10), true, false),
        item("z", "Idle (+19)", MenuAction::Renice(19), true, false),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: Caps = Caps { clipboard: true, finder: true, terminal: true, signals: true, renice: true };
    const NO_GUI: Caps = Caps { clipboard: false, finder: false, terminal: false, signals: true, renice: true };
    const NO_SIG: Caps = Caps { clipboard: true, finder: true, terminal: true, signals: false, renice: true };
    const NO_RENICE: Caps = Caps { clipboard: true, finder: true, terminal: true, signals: true, renice: false };

    fn has(items: &[MenuItem], a: &MenuAction) -> bool {
        items.iter().any(|i| &i.action == a)
    }
    fn find<'a>(items: &'a [MenuItem], a: &MenuAction) -> &'a MenuItem {
        items.iter().find(|i| &i.action == a).expect("action present")
    }

    #[test]
    fn process_menu_has_core_actions() {
        let m = build_process(ALL, true, true);
        assert!(has(&m, &MenuAction::Inspect));
        assert!(has(&m, &MenuAction::OpenKill));
        assert!(has(&m, &MenuAction::Suspend));
        assert!(has(&m, &MenuAction::Resume));
        assert!(has(&m, &MenuAction::CopyPid));
        assert!(find(&m, &MenuAction::OpenReniceSubmenu).opens_submenu);
    }

    #[test]
    fn process_reveal_open_disabled_without_metadata() {
        let m = build_process(ALL, false, true);
        assert!(!find(&m, &MenuAction::RevealExe).enabled);
        let m2 = build_process(ALL, true, false);
        assert!(!find(&m2, &MenuAction::OpenEditor).enabled);
    }

    #[test]
    fn process_gui_gated_by_caps() {
        let m = build_process(NO_GUI, true, true);
        assert!(!find(&m, &MenuAction::CopyPid).enabled);
        assert!(!find(&m, &MenuAction::RevealExe).enabled);
        assert!(!find(&m, &MenuAction::OpenTerminal).enabled);
    }

    #[test]
    fn process_signals_and_renice_gated() {
        let m = build_process(NO_SIG, true, true);
        assert!(!find(&m, &MenuAction::OpenKill).enabled);
        assert!(!find(&m, &MenuAction::Suspend).enabled);
        assert!(!find(&m, &MenuAction::Resume).enabled);
        let m2 = build_process(NO_RENICE, true, true);
        assert!(!find(&m2, &MenuAction::OpenReniceSubmenu).enabled);
    }

    #[test]
    fn system_bucket_has_no_kill_all() {
        let m = build_bucket(ALL, BucketKind::SystemOrUnknown, 5, false, false, false);
        assert!(!has(&m, &MenuAction::OpenKill));
        assert!(has(&m, &MenuAction::ToggleCollapse));
        assert!(has(&m, &MenuAction::Focus));
    }

    #[test]
    fn repo_bucket_has_open_in_and_kill() {
        let m = build_bucket(ALL, BucketKind::RepoOrCwd, 3, true, false, false);
        assert!(has(&m, &MenuAction::OpenKill));
        assert!(has(&m, &MenuAction::OpenEditor));
        assert!(has(&m, &MenuAction::OpenTerminal));
        assert!(has(&m, &MenuAction::CopyPath));
    }

    #[test]
    fn bucket_kill_all_gated_by_signals() {
        let m = build_bucket(NO_SIG, BucketKind::RepoOrCwd, 3, true, false, false);
        assert!(!find(&m, &MenuAction::OpenKill).enabled);
    }

    #[test]
    fn bundle_reveal_disabled_without_app_path() {
        let m = build_bucket(ALL, BucketKind::Bundle, 2, false, false, false);
        assert!(!find(&m, &MenuAction::RevealDir).enabled);
        let m2 = build_bucket(ALL, BucketKind::Bundle, 2, false, true, false);
        assert!(find(&m2, &MenuAction::RevealDir).enabled);
    }

    #[test]
    fn collapse_label_reflects_state() {
        let expanded = build_bucket(ALL, BucketKind::RepoOrCwd, 1, true, false, false);
        assert_eq!(expanded[0].label, "Collapse");
        let collapsed = build_bucket(ALL, BucketKind::RepoOrCwd, 1, true, false, true);
        assert_eq!(collapsed[0].label, "Expand");
    }

    #[test]
    fn all_root_menu_is_expand_collapse() {
        let m = build_all();
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].action, MenuAction::ExpandAll);
        assert_eq!(m[1].action, MenuAction::CollapseAll);
    }

    #[test]
    fn renice_presets_map_to_values() {
        let m = build_renice();
        assert_eq!(m[0].action, MenuAction::Renice(-10));
        assert_eq!(m[1].action, MenuAction::Renice(0));
        assert_eq!(m[2].action, MenuAction::Renice(10));
        assert_eq!(m[3].action, MenuAction::Renice(19));
    }

    #[test]
    fn nav_skips_disabled_and_clamps() {
        let items = vec![
            item("a", "a", MenuAction::Inspect, true, false),
            item("b", "b", MenuAction::CopyPid, false, false),
            item("c", "c", MenuAction::Suspend, true, false),
        ];
        let mut lvl = MenuLevel::new(items, 0, 0);
        assert_eq!(lvl.selected, 0);
        lvl.nav(1);
        assert_eq!(lvl.selected, 2, "skips the disabled middle item");
        lvl.nav(1);
        assert_eq!(lvl.selected, 2, "clamps at the end");
        lvl.nav(-1);
        assert_eq!(lvl.selected, 0);
    }

    #[test]
    fn outcome_mapping() {
        assert_eq!(outcome_for(&MenuAction::OpenReniceSubmenu), Outcome::Submenu);
        assert_eq!(outcome_for(&MenuAction::OpenKill), Outcome::KillPicker);
        assert_eq!(outcome_for(&MenuAction::Inspect), Outcome::Close);
    }

    #[test]
    fn push_pop_levels() {
        let mut cm = ContextMenu::new(Selection::All, build_all(), 0, 0);
        assert!(!cm.pop(), "root cannot pop");
        cm.push(build_renice(), 5, 5);
        assert_eq!(cm.levels.len(), 2);
        assert!(cm.pop());
        assert_eq!(cm.levels.len(), 1);
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test menu::`
Expected: all `menu::tests::*` pass (14 tests).

- [ ] **Step 4: Commit**

```bash
git add src/menu.rs src/main.rs
git commit -m "feat(menu): pure context-menu model, builders, navigation"
```

---

### Task 3: `actions.rs` — side effects with a testable command seam

**Files:**
- Create: `src/actions.rs`
- Modify: `src/main.rs` (add `mod actions;`)

**Interfaces:**
- Consumes: `crate::menu::Caps`.
- Produces:
  - `ExternalCfg { editor: Option<String>, terminal: String }` (Default `{ None, "Terminal" }`)
  - `caps() -> Caps`
  - `ShellAction { RevealDir(PathBuf), RevealFile(PathBuf), Editor(PathBuf), Terminal(PathBuf) }`
  - `shell_command(&ShellAction, &ExternalCfg) -> Option<Command>`
  - `run(ShellAction, &ExternalCfg) -> Result<(), String>`
  - `copy_to_clipboard(&str) -> Result<(), String>`
  - `renice(u32, i32) -> Result<(), String>`
  - `send_signal(u32, sysinfo::Signal)`, `send_signal_many(&[u32], sysinfo::Signal)`

- [ ] **Step 1: Register the module**

In `src/main.rs`, add `mod actions;` as the first module:

```rust
mod actions;
mod app;
mod collector;
```

- [ ] **Step 2: Write `src/actions.rs`**

```rust
//! Side-effecting helpers for context-menu actions. Command *construction*
//! (`shell_command`) is separated from *execution* (`run`) so the argv is unit
//! testable without spawning processes. External shell-outs use macOS `open`
//! and `pbcopy`; `caps()` reports what the current platform supports so the
//! menu disables the rest.

use std::path::PathBuf;
use std::process::Command;

use crate::menu::Caps;

#[derive(Clone, Debug)]
pub struct ExternalCfg {
    /// macOS application name for "Open in editor" via `open -a`
    /// (e.g. "Visual Studio Code", "Cursor"). When `None`/empty, fall back to
    /// `open -t` (the default text-edit app). A GUI app — never a TTY editor.
    pub editor: Option<String>,
    /// macOS application name for `open -a` in "Open in terminal".
    pub terminal: String,
}

impl Default for ExternalCfg {
    fn default() -> Self {
        Self { editor: None, terminal: "Terminal".into() }
    }
}

/// Platform capabilities. macOS supports everything; other Unix keeps signals
/// and renice but not the macOS GUI shell-outs; non-Unix supports none.
pub fn caps() -> Caps {
    #[cfg(target_os = "macos")]
    {
        Caps { clipboard: true, finder: true, terminal: true, signals: true, renice: true }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Caps { clipboard: false, finder: false, terminal: false, signals: true, renice: true }
    }
    #[cfg(not(unix))]
    {
        Caps { clipboard: false, finder: false, terminal: false, signals: false, renice: false }
    }
}

pub enum ShellAction {
    RevealDir(PathBuf),
    RevealFile(PathBuf),
    Editor(PathBuf),
    Terminal(PathBuf),
}

/// Build the macOS `open` command for a shell action. Always returns `Some` —
/// every variant maps to an `open` invocation (the editor is GUI-launched, so
/// it never touches the TTY).
pub fn shell_command(action: &ShellAction, cfg: &ExternalCfg) -> Option<Command> {
    let mut c = Command::new("open");
    match action {
        ShellAction::RevealDir(p) => {
            c.arg(p);
        }
        ShellAction::RevealFile(p) => {
            c.arg("-R").arg(p);
        }
        ShellAction::Terminal(p) => {
            c.arg("-a").arg(&cfg.terminal).arg(p);
        }
        ShellAction::Editor(p) => match cfg.editor.as_ref().filter(|s| !s.is_empty()) {
            Some(app) => {
                c.arg("-a").arg(app).arg(p);
            }
            None => {
                c.arg("-t").arg(p);
            }
        },
    }
    Some(c)
}

/// Execute a shell action and wait for `open` to return (it launches promptly).
/// A non-zero exit (e.g. unknown app) becomes a status-line error.
pub fn run(action: ShellAction, cfg: &ExternalCfg) -> Result<(), String> {
    let mut cmd = shell_command(&action, cfg).ok_or_else(|| "unsupported".to_string())?;
    let status = cmd.status().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("command failed".into())
    }
}

/// Copy text to the macOS pasteboard via `pbcopy`.
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        use std::process::Stdio;
        let mut child = Command::new("pbcopy")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?;
        child
            .stdin
            .as_mut()
            .ok_or_else(|| "no stdin".to_string())?
            .write_all(text.as_bytes())
            .map_err(|e| e.to_string())?;
        child.wait().map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = text;
        Err("clipboard unsupported".into())
    }
}

/// Set a process's scheduling niceness. Negative values usually require
/// elevated privileges; failure returns the OS error text for the status line.
#[cfg(unix)]
pub fn renice(pid: u32, niceness: i32) -> Result<(), String> {
    // setpriority returns 0 on success, -1 on error (errno set).
    let rc = unsafe { libc::setpriority(libc::PRIO_PROCESS, pid as libc::id_t, niceness) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().to_string())
    }
}

#[cfg(not(unix))]
pub fn renice(_pid: u32, _niceness: i32) -> Result<(), String> {
    Err("unsupported".into())
}

/// Send a signal to one process.
pub fn send_signal(pid: u32, sig: sysinfo::Signal) {
    send_signal_many(&[pid], sig);
}

/// Send a signal to many processes with a single system refresh.
pub fn send_signal_many(pids: &[u32], sig: sysinfo::Signal) {
    use sysinfo::{Pid, ProcessesToUpdate};
    let ids: Vec<Pid> = pids.iter().map(|p| Pid::from_u32(*p)).collect();
    if ids.is_empty() {
        return;
    }
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&ids), true);
    for id in &ids {
        if let Some(proc) = sys.process(*id) {
            proc.kill_with(sig);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(action: &ShellAction, cfg: &ExternalCfg) -> (String, Vec<String>) {
        let cmd = shell_command(action, cfg).expect("command built");
        let prog = cmd.get_program().to_string_lossy().into_owned();
        let args = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        (prog, args)
    }

    #[test]
    fn reveal_dir_argv() {
        let (prog, args) = argv(&ShellAction::RevealDir("/tmp/x".into()), &ExternalCfg::default());
        assert_eq!(prog, "open");
        assert_eq!(args, vec!["/tmp/x"]);
    }

    #[test]
    fn reveal_file_argv() {
        let (prog, args) = argv(&ShellAction::RevealFile("/bin/ls".into()), &ExternalCfg::default());
        assert_eq!(prog, "open");
        assert_eq!(args, vec!["-R", "/bin/ls"]);
    }

    #[test]
    fn terminal_argv_uses_configured_app() {
        let cfg = ExternalCfg { editor: None, terminal: "iTerm".into() };
        let (prog, args) = argv(&ShellAction::Terminal("/repo".into()), &cfg);
        assert_eq!(prog, "open");
        assert_eq!(args, vec!["-a", "iTerm", "/repo"]);
    }

    #[test]
    fn editor_gui_launch_with_app() {
        let cfg = ExternalCfg { editor: Some("Visual Studio Code".into()), terminal: "Terminal".into() };
        let (prog, args) = argv(&ShellAction::Editor("/repo".into()), &cfg);
        assert_eq!(prog, "open");
        assert_eq!(args, vec!["-a", "Visual Studio Code", "/repo"]);
    }

    #[test]
    fn editor_default_uses_text_edit() {
        let (prog, args) = argv(&ShellAction::Editor("/repo".into()), &ExternalCfg::default());
        assert_eq!(prog, "open");
        assert_eq!(args, vec!["-t", "/repo"]);
    }

    #[test]
    fn caps_track_platform() {
        assert_eq!(caps().clipboard, cfg!(target_os = "macos"));
        assert_eq!(caps().signals, cfg!(unix));
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test actions::`
Expected: all 6 `actions::tests::*` pass.

- [ ] **Step 4: Commit**

```bash
git add src/actions.rs src/main.rs
git commit -m "feat(actions): GUI-safe open/clipboard/renice/signal with testable argv"
```

---

### Task 4: `config.rs` — `[external]` editor/terminal config

**Files:**
- Modify: `src/config.rs` (add field + struct + default + tests)

**Interfaces:**
- Produces: `Config.external: ExternalConfig`; `ExternalConfig { editor: String, terminal: String }` (Default `{ "", "Terminal" }`).

- [ ] **Step 1: Add the `external` field to `Config`**

In `src/config.rs`, add to the `Config` struct (after the `state` field at line 22):

```rust
    #[serde(default)]
    pub state: StateConfig,
    #[serde(default)]
    pub external: ExternalConfig,
}
```

- [ ] **Step 2: Add the struct + default helper**

Add after the `StateConfig` block (around line 91), before `fn default_model`:

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExternalConfig {
    /// macOS app for "Open in editor" via `open -a` (e.g. "Cursor").
    /// Empty => `open -t` (default text-edit app). A GUI app, not a TTY editor.
    #[serde(default)]
    pub editor: String,
    /// macOS app for `open -a` in "Open in terminal".
    #[serde(default = "default_terminal")]
    pub terminal: String,
}

impl Default for ExternalConfig {
    fn default() -> Self {
        Self { editor: String::new(), terminal: default_terminal() }
    }
}

fn default_terminal() -> String {
    "Terminal".into()
}
```

- [ ] **Step 3: Add tests**

Append to `src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_defaults_to_terminal() {
        let c = ExternalConfig::default();
        assert_eq!(c.terminal, "Terminal");
        assert!(c.editor.is_empty());
    }

    #[test]
    fn config_default_includes_external() {
        assert_eq!(Config::default().external.terminal, "Terminal");
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test config::`
Expected: both pass.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): add [external] editor/terminal settings"
```

---

### Task 5: `ui/context_menu.rs` — pure geometry helpers

**Files:**
- Create: `src/ui/context_menu.rs`
- Modify: `src/ui/mod.rs` (add `pub mod context_menu;`)

**Interfaces:**
- Produces: `MENU_W: u16`; `Hit { Outside, Border, Item(usize) }`; `place(col,row,w,h,sw,sh) -> Rect`; `submenu_origin(parent_col, parent_w, sub_w, screen_w) -> u16`; `menu_hit(rect, col, row, n_items) -> Hit`. (The `render` fn is added in Task 8, once `App` carries the state.)

- [ ] **Step 1: Register the submodule**

In `src/ui/mod.rs`:

```rust
mod chart;
pub mod context_menu;
mod drilldown;
```

- [ ] **Step 2: Write `src/ui/context_menu.rs` with geometry + tests**

```rust
//! Geometry + rendering for the anchored context-menu cascade. The geometry
//! helpers (`place`, `submenu_origin`, `menu_hit`) are pure and unit tested;
//! `render` is added once `App` carries the menu state.

use ratatui::layout::Rect;

/// Fixed popup width (columns) for all menu levels.
pub const MENU_W: u16 = 26;

/// Result of hit-testing a click against a popup rect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Outside,
    Border,
    Item(usize),
}

/// Clamp a `w`x`h` popup anchored at `(col, row)` to stay fully within a
/// `screen_w`x`screen_h` terminal: pull left if it would overflow the right
/// edge, up if it would overflow the bottom.
pub fn place(col: u16, row: u16, w: u16, h: u16, screen_w: u16, screen_h: u16) -> Rect {
    let w = w.min(screen_w);
    let h = h.min(screen_h);
    let x = if col + w <= screen_w { col } else { screen_w.saturating_sub(w) };
    let y = if row + h <= screen_h { row } else { screen_h.saturating_sub(h) };
    Rect::new(x, y, w, h)
}

/// Left-column for a submenu of width `sub_w`: to the right of the parent
/// normally, but flipped to the parent's left (no overlap) when the right side
/// has no room.
pub fn submenu_origin(parent_col: u16, parent_w: u16, sub_w: u16, screen_w: u16) -> u16 {
    if parent_col + parent_w + sub_w <= screen_w {
        parent_col + parent_w
    } else {
        parent_col.saturating_sub(sub_w)
    }
}

/// Classify a click against a popup `rect` containing `n_items` rows (between
/// the top and bottom border rows).
pub fn menu_hit(rect: Rect, col: u16, row: u16, n_items: u16) -> Hit {
    if col < rect.x || col >= rect.x + rect.width || row < rect.y || row >= rect.y + rect.height {
        return Hit::Outside;
    }
    if row == rect.y || row == rect.y + rect.height - 1 {
        return Hit::Border;
    }
    let idx = row - rect.y - 1;
    if idx < n_items {
        Hit::Item(idx as usize)
    } else {
        Hit::Border
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn place_fits_without_clamping() {
        let r = place(2, 3, 26, 12, 200, 50);
        assert_eq!((r.x, r.y, r.width, r.height), (2, 3, 26, 12));
    }

    #[test]
    fn place_pulls_in_at_right_and_bottom() {
        assert_eq!(place(190, 3, 26, 12, 200, 50).x, 174); // 200 - 26
        assert_eq!(place(2, 45, 26, 12, 200, 50).y, 38); // 50 - 12
    }

    #[test]
    fn submenu_opens_right_then_flips_left() {
        // Room on the right.
        assert_eq!(submenu_origin(10, 26, 26, 200), 36);
        // No room: flip to the left (ends exactly at parent_col, no overlap).
        assert_eq!(submenu_origin(180, 26, 26, 200), 154);
    }

    #[test]
    fn hit_classifies_rows() {
        let rect = Rect::new(5, 5, 26, 5); // rows 5..10; items at 6,7,8; borders 5 & 9
        assert_eq!(menu_hit(rect, 6, 6, 3), Hit::Item(0));
        assert_eq!(menu_hit(rect, 6, 8, 3), Hit::Item(2));
        assert_eq!(menu_hit(rect, 6, 5, 3), Hit::Border);
        assert_eq!(menu_hit(rect, 6, 9, 3), Hit::Border);
        assert_eq!(menu_hit(rect, 40, 6, 3), Hit::Outside);
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test context_menu::`
Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/ui/context_menu.rs src/ui/mod.rs
git commit -m "feat(ui): context-menu geometry helpers (place/submenu/hit)"
```

---

### Task 6: `app.rs` — KillMenu retarget, status line, state fields, thin delegators

**Files:**
- Modify: `src/app.rs` — `KillMenu` (line 200), `open_kill_menu` (437), `kill_with_signal` (476), `kill_pid`/`kill_pid_signal` (1220-1231); add fields, status helpers, delegators, free fns; geometry helpers.
- Modify: `src/ui/mod.rs:159` (multi-PID kill title).

**Interfaces:**
- Consumes: `crate::actions::{send_signal_many, ExternalCfg}`, `crate::menu::ContextMenu`.
- Produces:
  - `KillMenu { targets: Vec<u32>, name: String }`
  - fields: `status_msg: Option<(String, Instant)>`, `context_menu: Option<crate::menu::ContextMenu>`, `external: crate::actions::ExternalCfg`, `term_size: (u16, u16)`
  - `open_kill_menu_targets(&mut self, Vec<u32>, String)`
  - `set_status`, `expire_status`, `status_text`
  - `search_height(&self) -> u16`, `sidebar_list_top(&self) -> u16`, `selected_tree_index(&self) -> Option<usize>`
  - delegators: `context_menu_nav`, `context_menu_back`, `context_menu_pop_only`, `close_context_menu`, `open_context_menu`, `context_menu_select`, `context_menu_click`
  - `pub(crate) fn app_ancestor(&Path) -> Option<PathBuf>`, `pub fn kill_menu_title(&[u32], &str) -> String`

- [ ] **Step 1: Change `KillMenu` and add fields**

Replace the struct at lines 199-203:

```rust
#[derive(Clone, Debug)]
pub struct KillMenu {
    pub targets: Vec<u32>,
    pub name: String,
}
```

In the `App` struct (after `pub kill_menu: Option<KillMenu>,` at line 188) add:

```rust
    pub kill_menu: Option<KillMenu>,
    // transient status-line message (text, set-at instant)
    pub status_msg: Option<(String, Instant)>,
    // anchored context menu (None when closed)
    pub context_menu: Option<crate::menu::ContextMenu>,
    // editor/terminal config for external open actions
    pub external: crate::actions::ExternalCfg,
    // last-rendered terminal size (cols, rows); updated each render
    pub term_size: (u16, u16),
```

In `App::new()` (after `kill_menu: None,` at line 356) add:

```rust
            kill_menu: None,
            status_msg: None,
            context_menu: None,
            external: crate::actions::ExternalCfg::default(),
            term_size: (80, 24),
```

- [ ] **Step 2: Map config in `apply_config` and `to_config_patch`**

In `apply_config` (after `self.collapsed = ...` at line 278):

```rust
        self.external = crate::actions::ExternalCfg {
            editor: if cfg.external.editor.is_empty() {
                None
            } else {
                Some(cfg.external.editor.clone())
            },
            terminal: cfg.external.terminal.clone(),
        };
```

In `to_config_patch` (after the `out.state = ...` block near line 316):

```rust
        out.external = crate::config::ExternalConfig {
            editor: self.external.editor.clone().unwrap_or_default(),
            terminal: self.external.terminal.clone(),
        };
```

- [ ] **Step 3: Retarget the kill flow**

Change `open_kill_menu`'s final block (line 467-469):

```rust
        if let Some(pid) = pid {
            self.kill_menu = Some(KillMenu { targets: vec![pid], name });
        }
```

Add after `open_kill_menu` (after line 470):

```rust
    pub fn open_kill_menu_targets(&mut self, targets: Vec<u32>, name: String) {
        if targets.is_empty() {
            return;
        }
        self.kill_menu = Some(KillMenu { targets, name });
    }
```

Replace `kill_with_signal` (lines 476-481):

```rust
    pub fn kill_with_signal(&mut self, sig: sysinfo::Signal) {
        if let Some(m) = self.kill_menu.clone() {
            crate::actions::send_signal_many(&m.targets, sig);
            let n = m.targets.len();
            self.set_status(format!(
                "sent {} to {} proc{}",
                signal_label(sig),
                n,
                if n == 1 { "" } else { "s" }
            ));
        }
        self.kill_menu = None;
    }
```

Replace `kill_pid` / `kill_pid_signal` (lines 1220-1231) with:

```rust
fn kill_pid(pid: u32) {
    crate::actions::send_signal(pid, sysinfo::Signal::Term);
}

fn signal_label(sig: sysinfo::Signal) -> &'static str {
    use sysinfo::Signal::*;
    match sig {
        Term => "SIGTERM",
        Kill => "SIGKILL",
        Hangup => "SIGHUP",
        Interrupt => "SIGINT",
        Stop => "SIGSTOP",
        Continue => "SIGCONT",
        Quit => "SIGQUIT",
        User1 => "SIGUSR1",
        User2 => "SIGUSR2",
        _ => "signal",
    }
}
```

- [ ] **Step 4: Add status helpers, geometry helpers, and free fns**

Add inside `impl App` (e.g. after `close_kill_menu`):

```rust
    pub fn set_status(&mut self, msg: String) {
        self.status_msg = Some((msg, Instant::now()));
    }

    pub fn expire_status(&mut self) {
        if let Some((_, at)) = &self.status_msg {
            if at.elapsed().as_millis() > 2500 {
                self.status_msg = None;
            }
        }
    }

    pub fn status_text(&self) -> Option<&str> {
        self.status_msg.as_ref().map(|(s, _)| s.as_str())
    }

    pub fn search_height(&self) -> u16 {
        if self.search_active || !self.search_query.is_empty() {
            1
        } else {
            0
        }
    }

    /// First sidebar list row (header 1 + search bar + block top + header row).
    pub fn sidebar_list_top(&self) -> u16 {
        1 + self.search_height() + 2
    }

    pub fn selected_tree_index(&self) -> Option<usize> {
        self.tree_rows().iter().position(|r| r.selection == self.selection)
    }
```

Add these free functions near `kill_pid` (module scope):

```rust
/// Walk an executable path up to its nearest `.app` bundle directory, e.g.
/// `/Applications/Foo.app` for `/Applications/Foo.app/Contents/MacOS/Foo`.
pub(crate) fn app_ancestor(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut cur = exe;
    while let Some(parent) = cur.parent() {
        let is_app = parent
            .file_name()
            .map(|n| n.to_string_lossy().ends_with(".app"))
            .unwrap_or(false);
        if is_app {
            return Some(parent.to_path_buf());
        }
        cur = parent;
    }
    None
}

/// Title for the kill/signal picker: single PID vs. an aggregated set.
pub fn kill_menu_title(targets: &[u32], name: &str) -> String {
    if targets.len() == 1 {
        format!(" kill [{}] {}", targets[0], name)
    } else {
        format!(" kill {} procs · {}", targets.len(), name)
    }
}
```

- [ ] **Step 5: Add the menu delegators**

Add inside `impl App` (the heavy lifting lives in `menu_dispatch.rs`, added in Task 7):

```rust
    pub fn open_context_menu(&mut self, target: Selection, col: u16, row: u16) {
        crate::menu_dispatch::open(self, target, col, row);
    }

    pub fn context_menu_nav(&mut self, delta: i32) {
        if let Some(cm) = self.context_menu.as_mut() {
            cm.nav(delta);
        }
    }

    /// Esc/q: pop a submenu, or close the whole menu at the root.
    pub fn context_menu_back(&mut self) {
        let at_root = self.context_menu.as_mut().map(|cm| !cm.pop()).unwrap_or(true);
        if at_root {
            self.context_menu = None;
        }
    }

    /// h/Left: pop a submenu only; a no-op at the root (does not close).
    pub fn context_menu_pop_only(&mut self) {
        if let Some(cm) = self.context_menu.as_mut() {
            cm.pop();
        }
    }

    pub fn close_context_menu(&mut self) {
        self.context_menu = None;
    }

    pub fn context_menu_select(&mut self) {
        crate::menu_dispatch::select(self);
    }

    pub fn context_menu_click(&mut self, col: u16, row: u16) {
        crate::menu_dispatch::click(self, col, row);
    }
```

- [ ] **Step 6: Update the kill-menu title in the renderer**

In `src/ui/mod.rs`, `render_kill_menu` (lines 157-161), replace the first title line:

```rust
    let lines = vec![
        Line::from(Span::styled(
            crate::app::kill_menu_title(&menu.targets, &truncate_label(&menu.name, 28)),
            Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD),
        )),
```

- [ ] **Step 7: Add tests for the title + app_ancestor**

Add a test module at the end of `src/app.rs`:

```rust
#[cfg(test)]
mod ctxmenu_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn app_ancestor_finds_bundle() {
        assert_eq!(
            app_ancestor(Path::new("/Applications/Foo.app/Contents/MacOS/Foo")),
            Some(std::path::PathBuf::from("/Applications/Foo.app"))
        );
    }

    #[test]
    fn app_ancestor_none_for_plain_binary() {
        assert_eq!(app_ancestor(Path::new("/usr/bin/ls")), None);
    }

    #[test]
    fn kill_title_singular_and_plural() {
        assert_eq!(kill_menu_title(&[42], "claude"), " kill [42] claude");
        assert_eq!(kill_menu_title(&[1, 2, 3], "~/code/x"), " kill 3 procs · ~/code/x");
    }
}
```

NOTE: This task will not compile standalone because the delegators in Step 5 call `crate::menu_dispatch`, which is created in Task 7. Implement Task 7 immediately after, then build. (If your workflow requires each task to compile, merge Tasks 6 and 7 into one commit.)

- [ ] **Step 8: Commit**

```bash
git add src/app.rs src/ui/mod.rs
git commit -m "feat(app): retarget KillMenu to a PID set; menu state + delegators"
```

---

### Task 7: `menu_dispatch.rs` — open/select/click + action dispatch over `&mut App`

**Files:**
- Create: `src/menu_dispatch.rs`
- Modify: `src/main.rs` (add `mod menu_dispatch;`)

**Interfaces:**
- Consumes: `crate::app::{App, Selection, BucketKey, app_ancestor}`, `crate::menu::*`, `crate::actions::*`, `crate::ui::context_menu::{place, submenu_origin, menu_hit, Hit, MENU_W}`.
- Produces: `pub(crate) fn open/select/click(app, ...)`; `pub(crate) fn buckets_to_collapse(&[String], &str) -> Vec<String>`.

- [ ] **Step 1: Register the module**

In `src/main.rs`, add `mod menu_dispatch;` after `mod menu;`:

```rust
mod menu;
mod menu_dispatch;
mod netmon;
```

- [ ] **Step 2: Write `src/menu_dispatch.rs`**

```rust
//! Bridges the pure menu model to the live `App`: builds the menu for a target,
//! routes clicks, and runs each action's side effect. Kept out of `app.rs` to
//! avoid growing it; everything here operates on `&mut App` via its public API.

use std::path::PathBuf;

use crate::actions::{self, ShellAction};
use crate::app::{app_ancestor, App, BucketKey, Selection};
use crate::menu::{self, BucketKind, ContextMenu, MenuAction, Outcome};
use crate::ui::context_menu::{menu_hit, place, submenu_origin, Hit, MENU_W};

/// Build and open the context menu for `target`, anchored at `(col, row)`.
pub(crate) fn open(app: &mut App, target: Selection, col: u16, row: u16) {
    let caps = actions::caps();
    let items = match &target {
        Selection::All => menu::build_all(),
        Selection::Process(_, pid) => {
            let (has_exe, has_cwd) = app
                .process_by_pid(*pid)
                .map(|p| (p.exe.is_some(), p.cwd.is_some()))
                .unwrap_or((false, false));
            menu::build_process(caps, has_exe, has_cwd)
        }
        Selection::Bucket(label) => {
            let kind = match bucket_key(app, label) {
                Some(BucketKey::Repo(_)) | Some(BucketKey::Cwd(_)) => BucketKind::RepoOrCwd,
                Some(BucketKey::Bundle(_)) => BucketKind::Bundle,
                _ => BucketKind::SystemOrUnknown,
            };
            let n = bucket_pids(app, label).len();
            let has_dir = bucket_dir(app, label).is_some();
            let has_app = bucket_app_path(app, label).is_some();
            let collapsed = app.collapsed.contains(label);
            menu::build_bucket(caps, kind, n, has_dir, has_app, collapsed)
        }
    };
    app.context_menu = Some(ContextMenu::new(target, items, col, row));
}

/// Activate the selected item: open a submenu, hand off to the kill picker, or
/// run a terminal action (which first closes the menu).
pub(crate) fn select(app: &mut App) {
    let (action, target) = match app.context_menu.as_ref() {
        Some(cm) => match cm.selected_item() {
            Some(item) if item.enabled => (item.action.clone(), cm.target.clone()),
            _ => return,
        },
        None => return,
    };
    match menu::outcome_for(&action) {
        Outcome::Submenu => open_submenu(app),
        Outcome::KillPicker => {
            let (targets, name) = kill_target(app);
            app.context_menu = None;
            app.open_kill_menu_targets(targets, name);
        }
        Outcome::Close => {
            app.context_menu = None;
            run_action(app, action, target);
        }
    }
}

/// Route a left-click. Only the topmost level is hit-tested; clicking a visible
/// parent-level row counts as "outside" and closes the menu (acceptable for v1).
pub(crate) fn click(app: &mut App, col: u16, row: u16) {
    let (sw, sh) = app.term_size;
    let Some(level) = app.context_menu.as_ref().and_then(|cm| cm.levels.last()) else {
        return;
    };
    let n = level.items.len() as u16;
    let rect = place(level.origin_col, level.origin_row, MENU_W, n + 2, sw, sh);
    match menu_hit(rect, col, row, n) {
        Hit::Outside => app.context_menu = None,
        Hit::Border => {}
        Hit::Item(idx) => {
            let enabled = app
                .context_menu
                .as_ref()
                .and_then(|cm| cm.levels.last())
                .and_then(|l| l.items.get(idx))
                .map(|i| i.enabled)
                .unwrap_or(false);
            if enabled {
                if let Some(level) = app.context_menu.as_mut().and_then(|cm| cm.levels.last_mut()) {
                    level.selected = idx;
                }
                select(app);
            }
        }
    }
}

fn open_submenu(app: &mut App) {
    let items = menu::build_renice();
    let sw = app.term_size.0;
    if let Some(cm) = app.context_menu.as_mut() {
        if let Some(level) = cm.levels.last() {
            let col = submenu_origin(level.origin_col, MENU_W, MENU_W, sw);
            let row = level.origin_row + level.selected as u16;
            cm.push(items, col, row);
        }
    }
}

fn run_action(app: &mut App, action: MenuAction, target: Selection) {
    use MenuAction as A;
    let pid = if let Selection::Process(_, p) = &target { Some(*p) } else { None };
    let label = match &target {
        Selection::Bucket(l) | Selection::Process(l, _) => Some(l.clone()),
        Selection::All => None,
    };

    match action {
        A::Inspect => {
            if let Some(p) = pid {
                app.open_drilldown(p);
                app.ensure_drilldown_loaded_for_tab();
            }
        }
        A::Suspend => {
            if let Some(p) = pid {
                actions::send_signal(p, sysinfo::Signal::Stop);
                app.set_status(format!("sent SIGSTOP to {}", p));
            }
        }
        A::Resume => {
            if let Some(p) = pid {
                actions::send_signal(p, sysinfo::Signal::Continue);
                app.set_status(format!("sent SIGCONT to {}", p));
            }
        }
        A::Renice(n) => {
            if let Some(p) = pid {
                match actions::renice(p, n) {
                    Ok(()) => app.set_status(format!("reniced {} → {:+}", p, n)),
                    Err(e) => app.set_status(format!("renice failed ({})", e)),
                }
            }
        }
        A::CopyPid => {
            if let Some(p) = pid {
                copy(app, &p.to_string(), format!("copied pid {}", p));
            }
        }
        A::CopyCommand => {
            if let Some(p) = pid {
                let cmd = app
                    .process_by_pid(p)
                    .map(|s| if s.cmd.is_empty() { s.name.clone() } else { s.cmd.clone() })
                    .unwrap_or_default();
                copy(app, &cmd, "copied command".into());
            }
        }
        A::CopyPath => {
            if let Some(l) = &label {
                if let Some(dir) = bucket_dir(app, l) {
                    let text = dir.display().to_string();
                    copy(app, &text, "copied path".into());
                }
            }
        }
        A::RevealExe => {
            if let Some(p) = pid {
                if let Some(exe) = app.process_by_pid(p).and_then(|s| s.exe.clone()) {
                    shell(app, ShellAction::RevealFile(exe), "revealed in Finder");
                }
            }
        }
        A::RevealDir => {
            if let Some(l) = &label {
                if let Some(dir) = bucket_dir(app, l).or_else(|| bucket_app_path(app, l)) {
                    shell(app, ShellAction::RevealDir(dir), "revealed in Finder");
                }
            }
        }
        A::OpenEditor => {
            if let Some(dir) = target_dir(app, &target) {
                shell(app, ShellAction::Editor(dir), "opened in editor");
            }
        }
        A::OpenTerminal => {
            if let Some(dir) = target_dir(app, &target) {
                shell(app, ShellAction::Terminal(dir), "opened in terminal");
            }
        }
        A::ToggleCollapse => {
            if let Some(l) = label {
                if app.collapsed.contains(&l) {
                    app.collapsed.remove(&l);
                } else {
                    app.collapsed.insert(l);
                }
            }
        }
        A::Focus => {
            if let Some(l) = label {
                focus(app, &l);
            }
        }
        A::ExpandAll => app.collapsed.clear(),
        A::CollapseAll => {
            let labels: Vec<String> = app.buckets.iter().map(|b| b.key.label()).collect();
            for l in labels {
                app.collapsed.insert(l);
            }
        }
        // Handled before dispatch.
        A::OpenKill | A::OpenReniceSubmenu => {}
    }
}

fn copy(app: &mut App, text: &str, ok: String) {
    match actions::copy_to_clipboard(text) {
        Ok(()) => app.set_status(ok),
        Err(e) => app.set_status(format!("copy failed ({})", e)),
    }
}

fn shell(app: &mut App, action: ShellAction, ok: &str) {
    match actions::run(action, &app.external) {
        Ok(()) => app.set_status(ok.into()),
        Err(e) => app.set_status(e),
    }
}

fn target_dir(app: &App, target: &Selection) -> Option<PathBuf> {
    match target {
        Selection::Process(_, pid) => app.process_by_pid(*pid).and_then(|s| s.cwd.clone()),
        Selection::Bucket(l) => bucket_dir(app, l),
        Selection::All => None,
    }
}

fn focus(app: &mut App, label: &str) {
    let labels: Vec<String> = app.buckets.iter().map(|b| b.key.label()).collect();
    for l in buckets_to_collapse(&labels, label) {
        app.collapsed.insert(l);
    }
    app.collapsed.remove(label);
}

fn kill_target(app: &App) -> (Vec<u32>, String) {
    match app.context_menu.as_ref().map(|c| &c.target) {
        Some(Selection::Process(_, pid)) => {
            let name = app.process_by_pid(*pid).map(|p| p.name.clone()).unwrap_or_default();
            (vec![*pid], name)
        }
        Some(Selection::Bucket(label)) => (bucket_pids(app, label), label.clone()),
        _ => (Vec::new(), String::new()),
    }
}

fn bucket_key(app: &App, label: &str) -> Option<BucketKey> {
    app.buckets.iter().find(|b| b.key.label() == label).map(|b| b.key.clone())
}

fn bucket_pids(app: &App, label: &str) -> Vec<u32> {
    app.buckets
        .iter()
        .find(|b| b.key.label() == label)
        .map(|b| b.pids.clone())
        .unwrap_or_default()
}

fn bucket_dir(app: &App, label: &str) -> Option<PathBuf> {
    match bucket_key(app, label)? {
        BucketKey::Repo(p) | BucketKey::Cwd(p) => Some(p),
        _ => None,
    }
}

fn bucket_app_path(app: &App, label: &str) -> Option<PathBuf> {
    if !matches!(bucket_key(app, label)?, BucketKey::Bundle(_)) {
        return None;
    }
    for pid in bucket_pids(app, label) {
        if let Some(exe) = app.process_by_pid(pid).and_then(|p| p.exe.clone()) {
            if let Some(app_dir) = app_ancestor(&exe) {
                return Some(app_dir);
            }
        }
    }
    None
}

/// All bucket labels except `keep` — the set "Focus" collapses.
pub(crate) fn buckets_to_collapse(labels: &[String], keep: &str) -> Vec<String> {
    labels.iter().filter(|l| l.as_str() != keep).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_collapses_all_but_kept() {
        let labels = vec!["(system)".to_string(), "Foo.app (bundle)".to_string(), "~/code/x".to_string()];
        let got = buckets_to_collapse(&labels, "Foo.app (bundle)");
        assert_eq!(got, vec!["(system)".to_string(), "~/code/x".to_string()]);
    }
}
```

- [ ] **Step 3: Build and test**

Run: `cargo test`
Expected: compiles; `menu_dispatch::tests::focus_collapses_all_but_kept` and the Task-6 `ctxmenu_tests` pass alongside the rest.

- [ ] **Step 4: Commit**

```bash
git add src/menu_dispatch.rs src/main.rs
git commit -m "feat(menu): action dispatch + open/click over App in menu_dispatch"
```

---

### Task 8: `ui/mod.rs` — render the cascade, footer status, term_size

**Files:**
- Modify: `src/ui/context_menu.rs` (add `render`)
- Modify: `src/ui/mod.rs` — set `term_size`; call `context_menu::render`; footer status + `x menu` hint.

**Interfaces:**
- Consumes: `App::{context_menu, status_text, term_size}`, `crate::ui::context_menu::{place, MENU_W}`.
- Produces: `context_menu::render(f, area, app)`.

- [ ] **Step 1: Add `render` to `src/ui/context_menu.rs`**

Append:

```rust
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::app::App;

/// Draw every level in the menu stack as an anchored, edge-clamped popup.
pub fn render(f: &mut Frame, area: Rect, app: &App) {
    let Some(cm) = app.context_menu.as_ref() else {
        return;
    };
    for level in &cm.levels {
        let h = level.items.len() as u16 + 2;
        let rect = place(level.origin_col, level.origin_row, MENU_W, h, area.width, area.height);
        let lines: Vec<Line> = level
            .items
            .iter()
            .enumerate()
            .map(|(i, it)| {
                let style = if !it.enabled {
                    Style::default().fg(Color::DarkGray)
                } else if i == level.selected {
                    Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let suffix = if it.opens_submenu { " ▸" } else { "" };
                Line::from(Span::styled(format!(" {} {}{}", it.icon, it.label, suffix), style))
            })
            .collect();
        f.render_widget(Clear, rect);
        f.render_widget(
            Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Cyan))),
            rect,
        );
    }
}
```

- [ ] **Step 2: Set `term_size` and render the menu in `render()`**

In `src/ui/mod.rs`, at the top of `render` (right after `let size = f.area();` at line 16):

```rust
    let size = f.area();
    app.term_size = (size.width, size.height);
```

After the `kill_menu` block (lines 46-48) add:

```rust
    if app.kill_menu.is_some() {
        render_kill_menu(f, size, app);
    }
    if app.context_menu.is_some() {
        context_menu::render(f, size, app);
    }
```

- [ ] **Step 3: Footer status + `x menu` hint**

Replace `render_footer` (lines 625-638):

```rust
fn render_footer(f: &mut Frame, area: Rect, app: &App) {
    if let Some(msg) = app.status_text() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {} ", msg),
                Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
            ))),
            area,
        );
        return;
    }
    let hint = if app.search_active {
        "type to filter · enter commit · esc cancel · ↑↓ nav"
    } else {
        "j/k nav · enter drill · x menu · K kill · c/m/n/w sort · T therm · / search · space pause · ? help · q quit"
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray)))),
        area,
    );
}
```

- [ ] **Step 4: Build**

Run: `cargo build`
Expected: compiles.

- [ ] **Step 5: Commit**

```bash
git add src/ui/context_menu.rs src/ui/mod.rs
git commit -m "feat(ui): render context-menu cascade + footer status line"
```

---

### Task 9: `main.rs` — wire mouse + keyboard

**Files:**
- Modify: `src/main.rs` — `expire_status` per tick; context-menu key branch; `x` opener; right-click open (with modal guard); left-click-when-open.

**Interfaces:**
- Consumes: `App::{open_context_menu, context_menu_nav, context_menu_select, context_menu_back, context_menu_pop_only, context_menu_click, selected_tree_index, sidebar_list_top, expire_status}`.

- [ ] **Step 1: Expire status each tick**

Before the render block (line 162, `if last_render.elapsed() >= ...`):

```rust
        app.expire_status();
        if last_render.elapsed() >= Duration::from_millis(100) {
```

- [ ] **Step 2: Context-menu key branch**

After the kill-menu block (which ends at line 277), before the drill-down block (line 279), insert:

```rust
                    // Context menu hijacks input while open.
                    if app.context_menu.is_some() {
                        match key.code {
                            KeyCode::Esc | KeyCode::Char('q') => {
                                app.context_menu_back();
                                continue;
                            }
                            KeyCode::Char('j') | KeyCode::Down => {
                                app.context_menu_nav(1);
                                continue;
                            }
                            KeyCode::Char('k') | KeyCode::Up => {
                                app.context_menu_nav(-1);
                                continue;
                            }
                            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                                app.context_menu_select();
                                continue;
                            }
                            KeyCode::Char('h') | KeyCode::Left => {
                                app.context_menu_pop_only();
                                continue;
                            }
                            _ => {
                                continue;
                            }
                        }
                    }
```

- [ ] **Step 3: `x` opener in normal mode**

In the normal-mode `match key.code` block, add after the `KeyCode::Char('T')` arm (line 391):

```rust
                        KeyCode::Char('x') => {
                            if app.pane == crate::app::Pane::Sidebar {
                                if let Some(idx) = app.selected_tree_index() {
                                    let sel = app.selection.clone();
                                    let row = app.sidebar_list_top() + idx as u16;
                                    app.open_context_menu(sel, 2, row);
                                }
                            }
                        }
```

- [ ] **Step 4: Mouse — left-click-when-open + right-click open**

In `handle_mouse`, replace the existing `sidebar_list_top` local (line 425) so the geometry can't drift:

```rust
    let sidebar_list_top = app.sidebar_list_top();
```

Make the `MouseEventKind::Down(MouseButton::Left)` arm consume clicks while the menu is open (insert at the very top of that arm, before the drill-down close at line 456-460):

```rust
        MouseEventKind::Down(MouseButton::Left) => {
            // An open context menu consumes the click: pick an item or close.
            if app.context_menu.is_some() {
                app.context_menu_click(ev.column, ev.row);
                return;
            }
            // Close the drill-down modal if the user clicks outside it.
            if app.drilldown_pid.is_some() {
                app.close_drilldown();
                return;
            }
```

Add a right-button arm before the final `_ => {}` (line 578):

```rust
        MouseEventKind::Down(MouseButton::Right) => {
            // Don't stack the menu under another modal.
            if app.kill_menu.is_some()
                || app.show_thermal
                || app.search_active
                || app.drilldown_pid.is_some()
            {
                return;
            }
            if ev.column < sidebar_w
                && ev.row >= sidebar_list_top
                && ev.row < term_h - footer
            {
                let idx = (ev.row - sidebar_list_top) as usize;
                let rows = app.tree_rows();
                if let Some(row) = rows.get(idx) {
                    app.selection = row.selection.clone();
                    app.pane = Pane::Sidebar;
                    let sel = row.selection.clone();
                    app.open_context_menu(sel, ev.column + 1, ev.row + 1);
                }
            }
        }
```

- [ ] **Step 5: Build and test**

Run: `cargo build && cargo test`
Expected: compiles; all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs
git commit -m "feat(main): wire right-click + x to open the context menu"
```

---

### Task 10: End-to-end verification + docs

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Lint (now meaningful, after Task 1)**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings. Fix any new ones (unused imports, etc.) before continuing.

- [ ] **Step 2: Full test run**

Run: `cargo test`
Expected: all pass.

- [ ] **Step 3: Manual smoke test**

Run: `cargo run`

Verify, observing the terminal stays intact throughout:
1. Right-click a **repo/cwd project** → Collapse, Kill all N…, Focus, Reveal/Open/Copy path.
2. Right-click a **process** → Inspect, Kill…, Suspend, Resume, Renice ▸, Copy PID/command, Reveal/Open.
3. Right-click an **app-bundle** bucket → Reveal app in Finder opens the `.app` (confirms exe→`.app` resolution).
4. Right-click `(system)` → only Collapse + Focus (no Kill all).
5. **Inspect** via Enter → menu closes, drill-down opens **and responds to `j/k/Esc`** (close-on-activate).
6. **Renice ▸** → cascade opens to the side (right, or left near the screen edge — no overlap); pick **Low (+10)** → status `reniced <pid> → +10`.
7. **Open cwd in editor** → a GUI editor window opens and the **TUI is still intact** after it launches (the critical TUI-safety check).
8. **Open cwd in terminal** → terminal app opens at the dir.
9. **Copy PID** → status `copied pid <n>`; paste to confirm.
10. Press `x` on the focused row → menu opens there; `j/k` navigate; `h`/`←` is a no-op at root; `Esc` closes.
11. Open a modal (drill-down or kill menu), then right-click the sidebar → **no second menu appears** (modal guard).
12. Right-click near the right/bottom edge → popup stays fully on-screen.

- [ ] **Step 4: Update the README**

In `README.md`, add to the documented keys/features (match existing style), e.g.: "Right-click (or `x`) a sidebar row for a context menu: inspect, kill/suspend/renice, copy, reveal/open."

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs: document sidebar context menu (right-click / x)"
```

---

## Self-Review

**Spec coverage:**
- Anchored popup + `x` trigger → Tasks 5, 6, 7, 9. ✓
- Cascade renice submenu, true left-flip near edge → `build_renice` (T2), `submenu_origin` (T5), `open_submenu` (T7), render loop (T8). ✓
- Kill… reuses centered picker, retargeted to PID sets → `KillMenu { targets }` + `kill_menu_title` (T6), `kill_target`/`open_kill_menu_targets` (T6/T7), batched `send_signal_many` (T3). ✓
- Context-sensitive content (process / repo-cwd / bundle / system / all) → `build_*` (T2), `open` resolution (T7). ✓
- Bundle reveal path from member exe → `app_ancestor` (T6) + `bucket_app_path` (T7). ✓
- Metadata from cached `ProcSample` via `process_by_pid` (no fresh `System`) → throughout T7. ✓
- Close-on-activate (Inspect vs drill-down) → `outcome_for` + `select` (T2/T7), manual check (T10.3 step 5). ✓
- TUI-safe editor (GUI launch) → `shell_command` Editor (T3), `caps.finder` gate (T2), manual check (T10.3 step 7). ✓
- Platform gating incl. signals/renice → `Caps` + `caps()` (T2/T3), builder `enabled` flags + tests (T2). ✓
- `h`/`←` pop-only vs `Esc`/`q` close → `context_menu_pop_only`/`context_menu_back` (T6), key branch (T9). ✓
- Modal guard on right-click → T9.4. ✓
- Status line feedback (incl. signal name, clipboard/renice errors) + `x menu` hint → `set_status`/`expire_status` + `signal_label` (T6), `copy`/`shell`/renice handling (T7), footer (T8), expiry tick (T9). ✓
- Config `[external]` editor/terminal → T4, mapped in `apply_config`/`to_config_patch` (T6). ✓
- File-size constraint → dispatch in `menu_dispatch.rs` (T7); `app.rs` gains only fields + thin delegators (T6). ✓
- Pre-existing clippy fixed so the gate is real → T1. ✓
- Non-goals (no process-table right-click, no hover, no Quit/Force-quit) → not implemented, as intended. ✓

**Placeholder scan:** No TBD/TODO; every code step shows complete code. (Task 6 carries an explicit compile-ordering note that it pairs with Task 7 — not a placeholder.) ✓

**Type consistency:** `Caps { clipboard, finder, terminal, signals, renice }` identical in T2 (def/use), T3 (`caps()`). `MenuAction`/`Outcome`/`BucketKind` consistent across T2, T7, T8. `KillMenu { targets, name }` consistent across T6 (struct/title/`open_kill_menu_targets`), T7 (`kill_target`). `ExternalCfg { editor: Option<String>, terminal: String }` (actions, T3) vs `ExternalConfig { editor: String, terminal: String }` (config, T4) bridged in `apply_config`/`to_config_patch` (T6). `place`/`submenu_origin`/`menu_hit`/`Hit`/`MENU_W` defined in T5, consumed in T7 (`click`/`open_submenu`) and T8 (`render`). `app_ancestor` (T6) consumed in T7. `sidebar_list_top()` defined T6, used T9 in both the `x` arm and `handle_mouse` (no drift). ✓
