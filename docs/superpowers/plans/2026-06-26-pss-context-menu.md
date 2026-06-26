# pss Left-Sidebar Context Menu Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a right-click (and `x`-key) context menu to the left sidebar that acts on the hovered project bucket or process — inspect, kill/suspend/renice, copy, reveal/open — via an anchored popup with a cascade renice submenu.

**Architecture:** A pure `menu.rs` (model + builders + navigation, unit-tested) describes the menu; `actions.rs` performs side effects behind a testable command-construction seam; `ui/context_menu.rs` renders the anchored cascade; `app.rs` holds the `ContextMenu` state and dispatches actions; `main.rs` wires mouse/keyboard. The existing kill/signal picker is reused, retargeted from a single PID to a PID set.

**Tech Stack:** Rust, ratatui + crossterm (TUI), sysinfo (process control + signals), libc (renice), toml/serde (config). Target platform: macOS (external shell-outs gated; non-macOS items disabled).

## Global Constraints

- **Platform gating:** External actions (clipboard/reveal/terminal) are macOS-only in v1; `actions::caps()` returns all-false off macOS and the builder disables those items. Signal/renice paths use `#[cfg(unix)]`, matching `src/app.rs` (`current_uid`, signal sending). Non-macOS builds must still compile.
- **Reuse the cache:** Process exe/cwd/cmd come from the cached `ProcSample` via `App::process_by_pid(pid)` (`src/collector.rs:69-90`). Do **not** create a fresh `sysinfo::System` to read metadata.
- **File size:** Keep new files focused; do not pile logic onto the already-large `src/app.rs` (~1249 lines) — new logic goes in `menu.rs` / `actions.rs` / `ui/context_menu.rs`.
- **Close-on-activate:** Activating any terminal (non-submenu) action clears `context_menu` before running, so the drill-down modal (lower in input precedence) receives its own keys.
- **Conventional Commits** for every commit.
- **Selection reuse:** The menu's target is the existing `crate::app::Selection` enum — no parallel target enum.

## File Structure

| File | Responsibility | Change |
|------|----------------|--------|
| `src/menu.rs` | Pure menu model, builders, navigation, action→outcome mapping | Create |
| `src/actions.rs` | Side effects: clipboard, reveal/open, renice, signal; testable argv | Create |
| `src/ui/context_menu.rs` | Anchored cascade rendering + `place()` clamping | Create |
| `src/config.rs` | `[external]` editor/terminal config | Modify |
| `src/app.rs` | `KillMenu` retarget, `ContextMenu` state, helpers, dispatch, status line | Modify |
| `src/ui/mod.rs` | Render context menu, multi-PID kill title, footer status + hint | Modify |
| `src/main.rs` | Module decls, right-click + `x` open, key/mouse routing, status expiry | Modify |

---

### Task 1: `menu.rs` — pure menu model, builders, navigation

**Files:**
- Create: `src/menu.rs`
- Modify: `src/main.rs:1-9` (add `mod menu;`)

**Interfaces:**
- Consumes: `crate::app::Selection` (existing enum: `All`, `Bucket(String)`, `Process(String, u32)`).
- Produces:
  - `Caps { clipboard: bool, finder: bool, terminal: bool }`
  - `BucketKind { RepoOrCwd, Bundle, SystemOrUnknown }`
  - `MenuAction` (enum, `#[derive(Clone, PartialEq, Eq, Debug)]`)
  - `MenuItem { icon: &'static str, label: String, action: MenuAction, enabled: bool, opens_submenu: bool }`
  - `MenuLevel { items: Vec<MenuItem>, selected: usize, origin_col: u16, origin_row: u16 }` with `new(items, col, row)` and `nav(&mut self, delta: i32)`
  - `ContextMenu { target: Selection, levels: Vec<MenuLevel> }` with `new`, `nav`, `selected_item`, `push`, `pop`
  - `Outcome { Close, Submenu, KillPicker }` and `outcome_for(&MenuAction) -> Outcome`
  - `build_process(caps, has_exe, has_cwd) -> Vec<MenuItem>`
  - `build_bucket(caps, kind, n_pids, has_dir, has_app_path, collapsed) -> Vec<MenuItem>`
  - `build_all() -> Vec<MenuItem>`
  - `build_renice() -> Vec<MenuItem>`

- [ ] **Step 1: Register the module**

In `src/main.rs`, add `mod menu;` to the module list (after `mod llm;`):

```rust
mod app;
mod collector;
mod config;
mod details;
mod heuristics;
mod llm;
mod menu;
mod netmon;
mod thermal;
mod ui;
```

- [ ] **Step 2: Write `src/menu.rs` with the model, builders, navigation, and tests**

```rust
//! Pure model for the left-sidebar context menu: item builders, level
//! navigation, and the action→outcome mapping. No side effects, no `App`
//! access beyond the plain-data `Selection` enum — everything here is unit
//! testable.

use crate::app::Selection;

/// Platform capabilities for the external (shell-out) actions. Produced by
/// `actions::caps()`; passed into builders so unsupported items render disabled.
#[derive(Clone, Copy, Debug)]
pub struct Caps {
    pub clipboard: bool,
    pub finder: bool,
    pub terminal: bool,
}

/// Which flavour of project bucket the menu targets.
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
    /// popped) so the caller can decide to close the whole menu.
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
        item("☠", "Kill…", MenuAction::OpenKill, true, false),
        item("‖", "Suspend", MenuAction::Suspend, true, false),
        item("▸", "Resume", MenuAction::Resume, true, false),
        item("⚖", "Renice", MenuAction::OpenReniceSubmenu, true, true),
        item("#", "Copy PID", MenuAction::CopyPid, caps.clipboard, false),
        item("⌗", "Copy command", MenuAction::CopyCommand, caps.clipboard, false),
        item("📁", "Reveal exe in Finder", MenuAction::RevealExe, caps.finder && has_exe, false),
        item("✎", "Open cwd in editor", MenuAction::OpenEditor, has_cwd, false),
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
        v.push(item("☠", &label, MenuAction::OpenKill, n_pids > 0, false));
    }
    v.push(item("⊙", "Focus", MenuAction::Focus, true, false));
    match kind {
        BucketKind::RepoOrCwd => {
            v.push(item("📁", "Reveal in Finder", MenuAction::RevealDir, caps.finder && has_dir, false));
            v.push(item("✎", "Open in editor", MenuAction::OpenEditor, has_dir, false));
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

    const ALL_CAPS: Caps = Caps { clipboard: true, finder: true, terminal: true };
    const NO_CAPS: Caps = Caps { clipboard: false, finder: false, terminal: false };

    fn has(items: &[MenuItem], a: &MenuAction) -> bool {
        items.iter().any(|i| &i.action == a)
    }
    fn find<'a>(items: &'a [MenuItem], a: &MenuAction) -> &'a MenuItem {
        items.iter().find(|i| &i.action == a).expect("action present")
    }

    #[test]
    fn process_menu_has_core_actions() {
        let m = build_process(ALL_CAPS, true, true);
        assert!(has(&m, &MenuAction::Inspect));
        assert!(has(&m, &MenuAction::OpenKill));
        assert!(has(&m, &MenuAction::Suspend));
        assert!(has(&m, &MenuAction::Resume));
        assert!(has(&m, &MenuAction::CopyPid));
        assert!(find(&m, &MenuAction::OpenReniceSubmenu).opens_submenu);
    }

    #[test]
    fn process_reveal_disabled_without_exe() {
        let m = build_process(ALL_CAPS, false, true);
        assert!(!find(&m, &MenuAction::RevealExe).enabled);
        let m2 = build_process(ALL_CAPS, true, false);
        assert!(!find(&m2, &MenuAction::OpenEditor).enabled);
    }

    #[test]
    fn process_clipboard_gated_by_caps() {
        let m = build_process(NO_CAPS, true, true);
        assert!(!find(&m, &MenuAction::CopyPid).enabled);
        assert!(!find(&m, &MenuAction::CopyCommand).enabled);
    }

    #[test]
    fn system_bucket_has_no_kill_all() {
        let m = build_bucket(ALL_CAPS, BucketKind::SystemOrUnknown, 5, false, false, false);
        assert!(!has(&m, &MenuAction::OpenKill));
        assert!(has(&m, &MenuAction::ToggleCollapse));
        assert!(has(&m, &MenuAction::Focus));
    }

    #[test]
    fn repo_bucket_has_open_in_and_kill() {
        let m = build_bucket(ALL_CAPS, BucketKind::RepoOrCwd, 3, true, false, false);
        assert!(has(&m, &MenuAction::OpenKill));
        assert!(has(&m, &MenuAction::OpenEditor));
        assert!(has(&m, &MenuAction::OpenTerminal));
        assert!(has(&m, &MenuAction::CopyPath));
    }

    #[test]
    fn bundle_reveal_disabled_without_app_path() {
        let m = build_bucket(ALL_CAPS, BucketKind::Bundle, 2, false, false, false);
        assert!(!find(&m, &MenuAction::RevealDir).enabled);
        let m2 = build_bucket(ALL_CAPS, BucketKind::Bundle, 2, false, true, false);
        assert!(find(&m2, &MenuAction::RevealDir).enabled);
    }

    #[test]
    fn collapse_label_reflects_state() {
        let expanded = build_bucket(ALL_CAPS, BucketKind::RepoOrCwd, 1, true, false, false);
        assert_eq!(expanded[0].label, "Collapse");
        let collapsed = build_bucket(ALL_CAPS, BucketKind::RepoOrCwd, 1, true, false, true);
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
        assert_eq!(outcome_for(&MenuAction::Renice(0)), Outcome::Close);
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

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test menu::`
Expected: all `menu::tests::*` pass (e.g. `test result: ok. 11 passed`).

- [ ] **Step 4: Commit**

```bash
git add src/menu.rs src/main.rs
git commit -m "feat(menu): pure context-menu model, builders, navigation"
```

---

### Task 2: `actions.rs` — side effects with a testable command seam

**Files:**
- Create: `src/actions.rs`
- Modify: `src/main.rs:1-10` (add `mod actions;`)

**Interfaces:**
- Consumes: `crate::menu::Caps`.
- Produces:
  - `ExternalCfg { editor: Option<String>, terminal: String }` (Default: `{ None, "Terminal" }`)
  - `caps() -> Caps`
  - `ShellAction { RevealDir(PathBuf), RevealFile(PathBuf), Editor(PathBuf), Terminal(PathBuf) }`
  - `shell_command(&ShellAction, &ExternalCfg) -> Option<Command>`
  - `run(ShellAction, &ExternalCfg) -> Result<(), String>`
  - `copy_to_clipboard(&str) -> Result<(), String>`
  - `renice(u32, i32) -> Result<(), String>`
  - `send_signal(u32, sysinfo::Signal)`

- [ ] **Step 1: Register the module**

In `src/main.rs`, add `mod actions;` as the first module (alphabetical, before `mod app;`):

```rust
mod actions;
mod app;
mod collector;
```

- [ ] **Step 2: Write `src/actions.rs` with the side-effect helpers and argv tests**

```rust
//! Side-effecting helpers for context-menu actions. Command *construction*
//! (`shell_command`) is separated from *execution* (`run`) so the argv is unit
//! testable without spawning processes. External shell-outs are macOS-only in
//! v1; `caps()` reports false elsewhere so the menu disables them.

use std::path::PathBuf;
use std::process::Command;

use crate::menu::Caps;

#[derive(Clone, Debug)]
pub struct ExternalCfg {
    /// Explicit editor command; when `None`, fall back to `$VISUAL`/`$EDITOR`.
    pub editor: Option<String>,
    /// macOS application name for `open -a` (e.g. "Terminal", "iTerm").
    pub terminal: String,
}

impl Default for ExternalCfg {
    fn default() -> Self {
        Self { editor: None, terminal: "Terminal".into() }
    }
}

/// Platform capabilities for the shell-out actions.
pub fn caps() -> Caps {
    #[cfg(target_os = "macos")]
    {
        Caps { clipboard: true, finder: true, terminal: true }
    }
    #[cfg(not(target_os = "macos"))]
    {
        Caps { clipboard: false, finder: false, terminal: false }
    }
}

pub enum ShellAction {
    RevealDir(PathBuf),
    RevealFile(PathBuf),
    Editor(PathBuf),
    Terminal(PathBuf),
}

/// Build the command for a shell action. Returns `None` only for `Editor` when
/// no editor is configured and neither `$VISUAL` nor `$EDITOR` is set.
pub fn shell_command(action: &ShellAction, cfg: &ExternalCfg) -> Option<Command> {
    match action {
        ShellAction::RevealDir(p) => {
            let mut c = Command::new("open");
            c.arg(p);
            Some(c)
        }
        ShellAction::RevealFile(p) => {
            let mut c = Command::new("open");
            c.arg("-R").arg(p);
            Some(c)
        }
        ShellAction::Terminal(p) => {
            let mut c = Command::new("open");
            c.arg("-a").arg(&cfg.terminal).arg(p);
            Some(c)
        }
        ShellAction::Editor(p) => {
            let editor = cfg
                .editor
                .clone()
                .or_else(|| std::env::var("VISUAL").ok())
                .or_else(|| std::env::var("EDITOR").ok())?;
            let mut c = Command::new(editor);
            c.arg(p);
            Some(c)
        }
    }
}

/// Execute a shell action, mapping failures to a status-line string.
pub fn run(action: ShellAction, cfg: &ExternalCfg) -> Result<(), String> {
    let is_editor = matches!(action, ShellAction::Editor(_));
    match shell_command(&action, cfg) {
        Some(mut cmd) => cmd.spawn().map(|_| ()).map_err(|e| e.to_string()),
        None if is_editor => Err("no $EDITOR set".into()),
        None => Err("unsupported".into()),
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
/// elevated privileges; failure returns a short reason for the status line.
#[cfg(unix)]
pub fn renice(pid: u32, niceness: i32) -> Result<(), String> {
    let rc = unsafe { libc::setpriority(libc::PRIO_PROCESS, pid as libc::id_t, niceness) };
    if rc == 0 {
        Ok(())
    } else {
        Err("not permitted".into())
    }
}

#[cfg(not(unix))]
pub fn renice(_pid: u32, _niceness: i32) -> Result<(), String> {
    Err("unsupported".into())
}

/// Send a signal to a process. Mirrors the existing one-shot refresh pattern
/// used by the kill path.
pub fn send_signal(pid: u32, sig: sysinfo::Signal) {
    use sysinfo::{Pid, ProcessesToUpdate};
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
    if let Some(proc) = sys.process(Pid::from_u32(pid)) {
        proc.kill_with(sig);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(action: &ShellAction, cfg: &ExternalCfg) -> (String, Vec<String>) {
        let cmd = shell_command(action, cfg).expect("command built");
        let prog = cmd.get_program().to_string_lossy().into_owned();
        let args = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
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
    fn editor_argv_prefers_configured_editor() {
        let cfg = ExternalCfg { editor: Some("vim".into()), terminal: "Terminal".into() };
        let (prog, args) = argv(&ShellAction::Editor("/repo".into()), &cfg);
        assert_eq!(prog, "vim");
        assert_eq!(args, vec!["/repo"]);
    }

    #[test]
    fn caps_track_platform() {
        assert_eq!(caps().clipboard, cfg!(target_os = "macos"));
        assert_eq!(caps().finder, cfg!(target_os = "macos"));
    }
}
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cargo test actions::`
Expected: all `actions::tests::*` pass (5 tests).

- [ ] **Step 4: Commit**

```bash
git add src/actions.rs src/main.rs
git commit -m "feat(actions): clipboard/reveal/open/renice/signal with testable argv seam"
```

---

### Task 3: `config.rs` — `[external]` editor/terminal config

**Files:**
- Modify: `src/config.rs:7-23` (add field), append `ExternalConfig` struct + default helper.

**Interfaces:**
- Produces: `Config.external: ExternalConfig`; `ExternalConfig { editor: String, terminal: String }` (Default: `{ "", "Terminal" }`).

- [ ] **Step 1: Add the `external` field to `Config`**

In `src/config.rs`, modify the `Config` struct (the field list ending at line 22) to add:

```rust
    #[serde(default)]
    pub state: StateConfig,
    #[serde(default)]
    pub external: ExternalConfig,
}
```

- [ ] **Step 2: Add the `ExternalConfig` struct and default helper**

Add after the `StateConfig` block (around line 91), before `fn default_model`:

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExternalConfig {
    /// Editor command for "Open in editor"; empty => $VISUAL/$EDITOR.
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

- [ ] **Step 3: Add a test for the default**

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
        let c = Config::default();
        assert_eq!(c.external.terminal, "Terminal");
    }
}
```

- [ ] **Step 4: Run the tests**

Run: `cargo test config::`
Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): add [external] editor/terminal settings"
```

---

### Task 4: `ui/context_menu.rs` — anchored placement helper

**Files:**
- Create: `src/ui/context_menu.rs`
- Modify: `src/ui/mod.rs:1-5` (add `pub mod context_menu;`)

**Interfaces:**
- Produces: `pub const MENU_W: u16`; `place(col, row, w, h, screen_w, screen_h) -> Rect`. (The `render` function is added in Task 7, once `App` has the `context_menu` field.)

- [ ] **Step 1: Register the submodule**

In `src/ui/mod.rs`, add to the module list:

```rust
mod chart;
pub mod context_menu;
mod drilldown;
mod processes;
mod recommendations;
pub mod sidebar;
```

- [ ] **Step 2: Write `src/ui/context_menu.rs` with `place()` + tests**

```rust
//! Rendering for the anchored context-menu cascade. `place()` clamps a popup
//! to stay on-screen (flip left / shift up near edges) and is unit tested; the
//! `render` function is added once `App` carries the menu state.

use ratatui::layout::Rect;

/// Fixed popup width (columns) for all menu levels.
pub const MENU_W: u16 = 26;

/// Clamp a popup of size `w`x`h` anchored at `(col, row)` so it stays fully
/// within a `screen_w`x`screen_h` terminal: flip left if it would overflow the
/// right edge, shift up if it would overflow the bottom.
pub fn place(col: u16, row: u16, w: u16, h: u16, screen_w: u16, screen_h: u16) -> Rect {
    let w = w.min(screen_w);
    let h = h.min(screen_h);
    let x = if col + w <= screen_w { col } else { screen_w.saturating_sub(w) };
    let y = if row + h <= screen_h { row } else { screen_h.saturating_sub(h) };
    Rect::new(x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_without_clamping() {
        let r = place(2, 3, 26, 12, 200, 50);
        assert_eq!((r.x, r.y, r.width, r.height), (2, 3, 26, 12));
    }

    #[test]
    fn flips_left_at_right_edge() {
        let r = place(190, 3, 26, 12, 200, 50);
        assert_eq!(r.x, 174); // 200 - 26
    }

    #[test]
    fn shifts_up_at_bottom_edge() {
        let r = place(2, 45, 26, 12, 200, 50);
        assert_eq!(r.y, 38); // 50 - 12
    }

    #[test]
    fn caps_size_to_screen() {
        let r = place(0, 0, 26, 12, 10, 5);
        assert_eq!((r.width, r.height), (10, 5));
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test context_menu::`
Expected: 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/ui/context_menu.rs src/ui/mod.rs
git commit -m "feat(ui): context-menu placement helper with edge clamping"
```

---

### Task 5: `app.rs` — retarget `KillMenu` to a PID set + status line

**Files:**
- Modify: `src/app.rs` — `KillMenu` struct (line 200), `open_kill_menu` (437), `kill_with_signal` (476), `kill_pid`/`kill_pid_signal` (1220-1231); add `status_msg` field + helpers; add `app_ancestor` free fn + `kill_menu_title` helper.
- Modify: `src/ui/mod.rs:159` (kill-menu title for the multi-PID case).

**Interfaces:**
- Consumes: `crate::actions::send_signal`.
- Produces:
  - `KillMenu { targets: Vec<u32>, name: String }`
  - `App::open_kill_menu_targets(&mut self, targets: Vec<u32>, name: String)`
  - `App::set_status(&mut self, msg: String)`, `App::expire_status(&mut self)`, `App::status_text(&self) -> Option<&str>`
  - free fn `app_ancestor(exe: &std::path::Path) -> Option<std::path::PathBuf>`
  - free fn `kill_menu_title(targets: &[u32], name: &str) -> String`

- [ ] **Step 1: Change the `KillMenu` struct**

In `src/app.rs` replace the struct at line 199-203:

```rust
#[derive(Clone, Debug)]
pub struct KillMenu {
    pub targets: Vec<u32>,
    pub name: String,
}
```

- [ ] **Step 2: Add the `status_msg` field**

In the `App` struct (near `pub kill_menu: Option<KillMenu>,` at line 188) add:

```rust
    // kill signal menu
    pub kill_menu: Option<KillMenu>,
    // transient status-line message (text, set-at instant)
    pub status_msg: Option<(String, Instant)>,
```

In `App::new()` (near `kill_menu: None,` at line 356) add:

```rust
            kill_menu: None,
            status_msg: None,
```

- [ ] **Step 3: Update `open_kill_menu`, add `open_kill_menu_targets`, loop in `kill_with_signal`**

Replace the body of `open_kill_menu` so it builds a single-target `KillMenu`. Change line 468:

```rust
        if let Some(pid) = pid {
            self.kill_menu = Some(KillMenu { targets: vec![pid], name });
        }
```

Add a new method right after `open_kill_menu` (after line 470):

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
            for pid in &m.targets {
                crate::actions::send_signal(*pid, sig);
            }
            let n = m.targets.len();
            self.set_status(format!(
                "sent signal to {} proc{}",
                n,
                if n == 1 { "" } else { "s" }
            ));
        }
        self.kill_menu = None;
    }
```

- [ ] **Step 4: Route the kill helpers through `actions::send_signal`**

Replace `kill_pid` / `kill_pid_signal` (lines 1220-1231) with a single helper:

```rust
fn kill_pid(pid: u32) {
    crate::actions::send_signal(pid, sysinfo::Signal::Term);
}
```

(Delete `kill_pid_signal`; `kill_selected` at line 1162 already calls `kill_pid`.)

- [ ] **Step 5: Add status helpers, `app_ancestor`, and `kill_menu_title` with tests**

Add these methods inside `impl App` (e.g. after `close_kill_menu`):

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
```

Add these free functions near `kill_pid` (module scope, not in `impl`):

```rust
/// Walk an executable path up to its nearest `.app` bundle directory.
/// Returns e.g. `/Applications/Foo.app` for
/// `/Applications/Foo.app/Contents/MacOS/Foo`; `None` when there is no `.app`
/// ancestor.
fn app_ancestor(exe: &std::path::Path) -> Option<std::path::PathBuf> {
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

Add a test module at the end of `src/app.rs` (or extend an existing one):

```rust
#[cfg(test)]
mod ctxmenu_tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn app_ancestor_finds_bundle() {
        let got = app_ancestor(Path::new("/Applications/Foo.app/Contents/MacOS/Foo"));
        assert_eq!(got, Some(std::path::PathBuf::from("/Applications/Foo.app")));
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

- [ ] **Step 6: Update the kill-menu title in the renderer**

In `src/ui/mod.rs`, `render_kill_menu` (line 157-161), replace the first title line so it uses the new helper and `targets`:

```rust
    let lines = vec![
        Line::from(Span::styled(
            crate::app::kill_menu_title(&menu.targets, &truncate_label(&menu.name, 28)),
            Style::default().fg(Color::LightYellow).add_modifier(Modifier::BOLD),
        )),
```

- [ ] **Step 7: Build and run the tests**

Run: `cargo test`
Expected: compiles; new `ctxmenu_tests::*` pass; existing tests still pass.

- [ ] **Step 8: Commit**

```bash
git add src/app.rs src/ui/mod.rs
git commit -m "feat(app): retarget KillMenu to a PID set; add status line + bundle-path helper"
```

---

### Task 6: `app.rs` — context-menu state, helpers, and dispatch

**Files:**
- Modify: `src/app.rs` — add `context_menu` + `external` fields; bucket helpers; `open_context_menu`; navigation/select/back/click; `run_menu_action`; `selected_tree_index`; wire `apply_config`/`to_config_patch`.

**Interfaces:**
- Consumes: `crate::menu::*`, `crate::actions::*`, `crate::ui::context_menu::{place, MENU_W}`, existing `process_by_pid`, `open_drilldown`, `ensure_drilldown_loaded_for_tab`, `tree_rows`, `collapsed`, `buckets`.
- Produces: `App::open_context_menu`, `context_menu_nav`, `context_menu_back`, `close_context_menu`, `context_menu_select`, `context_menu_click`, `run_menu_action`, `selected_tree_index`, `bucket_key`, `bucket_pids`, `bucket_dir`, `bucket_app_path`.

- [ ] **Step 1: Add the `context_menu` and `external` fields**

In the `App` struct (after the `status_msg` field added in Task 5) add:

```rust
    pub status_msg: Option<(String, Instant)>,
    // anchored context menu (None when closed)
    pub context_menu: Option<crate::menu::ContextMenu>,
    // editor/terminal config for external open actions
    pub external: crate::actions::ExternalCfg,
```

In `App::new()` (after `status_msg: None,`) add:

```rust
            status_msg: None,
            context_menu: None,
            external: crate::actions::ExternalCfg::default(),
```

- [ ] **Step 2: Map config in `apply_config` and `to_config_patch`**

In `apply_config` (after `self.collapsed = ...` at line 278) add:

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

In `to_config_patch` (before `out` is returned, after the `out.state = ...` block near line 316) add:

```rust
        out.external = crate::config::ExternalConfig {
            editor: self.external.editor.clone().unwrap_or_default(),
            terminal: self.external.terminal.clone(),
        };
```

- [ ] **Step 3: Add bucket + selection helpers**

Add inside `impl App` (near `selected_bucket_ref`, around line 887). Note `use std::path::PathBuf;` — confirm it's imported at the top of `app.rs`; if not, add it:

```rust
    pub fn bucket_key(&self, label: &str) -> Option<BucketKey> {
        self.buckets
            .iter()
            .find(|b| b.key.label() == label)
            .map(|b| b.key.clone())
    }

    pub fn bucket_pids(&self, label: &str) -> Vec<u32> {
        self.buckets
            .iter()
            .find(|b| b.key.label() == label)
            .map(|b| b.pids.clone())
            .unwrap_or_default()
    }

    pub fn bucket_dir(&self, label: &str) -> Option<PathBuf> {
        match self.bucket_key(label)? {
            BucketKey::Repo(p) | BucketKey::Cwd(p) => Some(p),
            _ => None,
        }
    }

    pub fn bucket_app_path(&self, label: &str) -> Option<PathBuf> {
        if !matches!(self.bucket_key(label)?, BucketKey::Bundle(_)) {
            return None;
        }
        for pid in self.bucket_pids(label) {
            if let Some(exe) = self.process_by_pid(pid).and_then(|p| p.exe.clone()) {
                if let Some(app) = app_ancestor(&exe) {
                    return Some(app);
                }
            }
        }
        None
    }

    pub fn selected_tree_index(&self) -> Option<usize> {
        self.tree_rows().iter().position(|r| r.selection == self.selection)
    }
```

- [ ] **Step 4: Add open / navigation / close**

Add inside `impl App`:

```rust
    pub fn open_context_menu(&mut self, target: Selection, col: u16, row: u16) {
        let caps = crate::actions::caps();
        let items = match &target {
            Selection::All => crate::menu::build_all(),
            Selection::Process(_, pid) => {
                let (has_exe, has_cwd) = self
                    .process_by_pid(*pid)
                    .map(|p| (p.exe.is_some(), p.cwd.is_some()))
                    .unwrap_or((false, false));
                crate::menu::build_process(caps, has_exe, has_cwd)
            }
            Selection::Bucket(label) => {
                let kind = match self.bucket_key(label) {
                    Some(BucketKey::Repo(_)) | Some(BucketKey::Cwd(_)) => {
                        crate::menu::BucketKind::RepoOrCwd
                    }
                    Some(BucketKey::Bundle(_)) => crate::menu::BucketKind::Bundle,
                    _ => crate::menu::BucketKind::SystemOrUnknown,
                };
                let n = self.bucket_pids(label).len();
                let has_dir = self.bucket_dir(label).is_some();
                let has_app = self.bucket_app_path(label).is_some();
                let collapsed = self.collapsed.contains(label);
                crate::menu::build_bucket(caps, kind, n, has_dir, has_app, collapsed)
            }
        };
        self.context_menu = Some(crate::menu::ContextMenu::new(target, items, col, row));
    }

    pub fn context_menu_nav(&mut self, delta: i32) {
        if let Some(cm) = self.context_menu.as_mut() {
            cm.nav(delta);
        }
    }

    pub fn context_menu_back(&mut self) {
        let at_root = self.context_menu.as_mut().map(|cm| !cm.pop()).unwrap_or(true);
        if at_root {
            self.context_menu = None;
        }
    }

    pub fn close_context_menu(&mut self) {
        self.context_menu = None;
    }
```

- [ ] **Step 5: Add select / submenu / click**

Add inside `impl App`:

```rust
    pub fn context_menu_select(&mut self) {
        let (action, target) = {
            let Some(cm) = self.context_menu.as_ref() else {
                return;
            };
            let Some(item) = cm.selected_item() else {
                return;
            };
            if !item.enabled {
                return;
            }
            (item.action.clone(), cm.target.clone())
        };
        match crate::menu::outcome_for(&action) {
            crate::menu::Outcome::Submenu => self.context_menu_open_submenu(),
            crate::menu::Outcome::KillPicker => {
                let (targets, name) = self.menu_kill_target();
                self.close_context_menu();
                self.open_kill_menu_targets(targets, name);
            }
            crate::menu::Outcome::Close => {
                self.close_context_menu();
                self.run_menu_action(action, target);
            }
        }
    }

    fn context_menu_open_submenu(&mut self) {
        let items = crate::menu::build_renice();
        if let Some(cm) = self.context_menu.as_mut() {
            if let Some(level) = cm.levels.last() {
                let col = level.origin_col + 24;
                let row = level.origin_row + level.selected as u16;
                cm.push(items, col, row);
            }
        }
    }

    pub fn context_menu_click(&mut self, col: u16, row: u16, screen_w: u16, screen_h: u16) {
        let rect = {
            let Some(cm) = self.context_menu.as_ref() else {
                return;
            };
            let Some(level) = cm.levels.last() else {
                return;
            };
            let h = level.items.len() as u16 + 2;
            crate::ui::context_menu::place(
                level.origin_col,
                level.origin_row,
                crate::ui::context_menu::MENU_W,
                h,
                screen_w,
                screen_h,
            )
        };
        let inside = col >= rect.x
            && col < rect.x + rect.width
            && row >= rect.y
            && row < rect.y + rect.height;
        if !inside {
            self.close_context_menu();
            return;
        }
        // Border rows (top/bottom) are not items.
        if row == rect.y || row == rect.y + rect.height - 1 {
            return;
        }
        let idx = (row - rect.y - 1) as usize;
        let ok = {
            let Some(cm) = self.context_menu.as_mut() else {
                return;
            };
            let Some(level) = cm.levels.last_mut() else {
                return;
            };
            if idx < level.items.len() && level.items[idx].enabled {
                level.selected = idx;
                true
            } else {
                false
            }
        };
        if ok {
            self.context_menu_select();
        }
    }

    fn menu_kill_target(&self) -> (Vec<u32>, String) {
        match self.context_menu.as_ref().map(|c| &c.target) {
            Some(Selection::Process(_, pid)) => {
                let name = self
                    .process_by_pid(*pid)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                (vec![*pid], name)
            }
            Some(Selection::Bucket(label)) => (self.bucket_pids(label), label.clone()),
            _ => (Vec::new(), String::new()),
        }
    }
```

- [ ] **Step 6: Add the action dispatcher**

Add inside `impl App`:

```rust
    pub fn run_menu_action(&mut self, action: crate::menu::MenuAction, target: Selection) {
        use crate::actions::{self, ShellAction};
        use crate::menu::MenuAction as A;

        let pid = if let Selection::Process(_, p) = &target { Some(*p) } else { None };
        let label = match &target {
            Selection::Bucket(l) | Selection::Process(l, _) => Some(l.clone()),
            Selection::All => None,
        };

        match action {
            A::Inspect => {
                if let Some(p) = pid {
                    self.open_drilldown(p);
                    self.ensure_drilldown_loaded_for_tab();
                }
            }
            A::Suspend => {
                if let Some(p) = pid {
                    actions::send_signal(p, sysinfo::Signal::Stop);
                    self.set_status(format!("sent SIGSTOP to {}", p));
                }
            }
            A::Resume => {
                if let Some(p) = pid {
                    actions::send_signal(p, sysinfo::Signal::Continue);
                    self.set_status(format!("sent SIGCONT to {}", p));
                }
            }
            A::Renice(n) => {
                if let Some(p) = pid {
                    match actions::renice(p, n) {
                        Ok(()) => self.set_status(format!("reniced {} → {:+}", p, n)),
                        Err(e) => self.set_status(format!("renice failed ({})", e)),
                    }
                }
            }
            A::CopyPid => {
                if let Some(p) = pid {
                    let _ = actions::copy_to_clipboard(&p.to_string());
                    self.set_status(format!("copied pid {}", p));
                }
            }
            A::CopyCommand => {
                if let Some(p) = pid {
                    let cmd = self
                        .process_by_pid(p)
                        .map(|s| if s.cmd.is_empty() { s.name.clone() } else { s.cmd.clone() })
                        .unwrap_or_default();
                    let _ = actions::copy_to_clipboard(&cmd);
                    self.set_status("copied command".into());
                }
            }
            A::CopyPath => {
                if let Some(l) = &label {
                    if let Some(dir) = self.bucket_dir(l) {
                        let _ = actions::copy_to_clipboard(&dir.display().to_string());
                        self.set_status("copied path".into());
                    }
                }
            }
            A::RevealExe => {
                if let Some(p) = pid {
                    if let Some(exe) = self.process_by_pid(p).and_then(|s| s.exe.clone()) {
                        self.run_shell(ShellAction::RevealFile(exe), "revealed in Finder");
                    }
                }
            }
            A::RevealDir => {
                if let Some(l) = &label {
                    if let Some(dir) = self.bucket_dir(l).or_else(|| self.bucket_app_path(l)) {
                        self.run_shell(ShellAction::RevealDir(dir), "revealed in Finder");
                    }
                }
            }
            A::OpenEditor => {
                if let Some(dir) = self.menu_target_dir(&target) {
                    self.run_shell(ShellAction::Editor(dir), "opened in editor");
                }
            }
            A::OpenTerminal => {
                if let Some(dir) = self.menu_target_dir(&target) {
                    self.run_shell(ShellAction::Terminal(dir), "opened in terminal");
                }
            }
            A::ToggleCollapse => {
                if let Some(l) = label {
                    if self.collapsed.contains(&l) {
                        self.collapsed.remove(&l);
                    } else {
                        self.collapsed.insert(l);
                    }
                }
            }
            A::Focus => {
                if let Some(l) = label {
                    self.focus_bucket(&l);
                }
            }
            A::ExpandAll => {
                self.collapsed.clear();
            }
            A::CollapseAll => {
                let labels: Vec<String> = self.buckets.iter().map(|b| b.key.label()).collect();
                for l in labels {
                    self.collapsed.insert(l);
                }
            }
            // Handled before dispatch.
            A::OpenKill | A::OpenReniceSubmenu => {}
        }
    }

    fn run_shell(&mut self, action: crate::actions::ShellAction, ok: &str) {
        match crate::actions::run(action, &self.external) {
            Ok(()) => self.set_status(ok.into()),
            Err(e) => self.set_status(e),
        }
    }

    fn menu_target_dir(&self, target: &Selection) -> Option<PathBuf> {
        match target {
            Selection::Process(_, pid) => self.process_by_pid(*pid).and_then(|s| s.cwd.clone()),
            Selection::Bucket(l) => self.bucket_dir(l),
            Selection::All => None,
        }
    }

    fn focus_bucket(&mut self, label: &str) {
        let others: Vec<String> = self
            .buckets
            .iter()
            .map(|b| b.key.label())
            .filter(|l| l != label)
            .collect();
        for l in others {
            self.collapsed.insert(l);
        }
        self.collapsed.remove(label);
    }
```

- [ ] **Step 7: Add a `focus_bucket` test**

Add to the `ctxmenu_tests` module created in Task 5:

```rust
    #[test]
    fn focus_collapses_other_buckets() {
        let mut app = App::new();
        app.buckets = vec![
            Bucket { key: BucketKey::System, cpu: 0.0, mem: 0, net_rx: 0, net_tx: 0, pids: vec![] },
            Bucket { key: BucketKey::Bundle("Foo.app".into()), cpu: 0.0, mem: 0, net_rx: 0, net_tx: 0, pids: vec![] },
        ];
        let target = "Foo.app (bundle)".to_string();
        app.focus_bucket(&target);
        assert!(app.collapsed.contains("(system)"));
        assert!(!app.collapsed.contains(&target));
    }
```

- [ ] **Step 8: Build and test**

Run: `cargo test`
Expected: compiles; `ctxmenu_tests::focus_collapses_other_buckets` passes alongside the rest.

- [ ] **Step 9: Commit**

```bash
git add src/app.rs
git commit -m "feat(app): context-menu state, bucket helpers, and action dispatch"
```

---

### Task 7: `ui/mod.rs` — render the menu, footer status, hint

**Files:**
- Modify: `src/ui/context_menu.rs` (add `render`)
- Modify: `src/ui/mod.rs` — call `context_menu::render`; footer status + `x menu` hint.

**Interfaces:**
- Consumes: `App::context_menu`, `App::status_text`, `crate::ui::context_menu::{place, MENU_W}`.
- Produces: `context_menu::render(f, area, app)`.

- [ ] **Step 1: Add `render` to `src/ui/context_menu.rs`**

Append to `src/ui/context_menu.rs`:

```rust
use ratatui::Frame;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::app::App;

/// Draw every level in the menu stack as an anchored, edge-clamped popup.
pub fn render(f: &mut Frame, area: ratatui::layout::Rect, app: &App) {
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
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let suffix = if it.opens_submenu { " ▸" } else { "" };
                Line::from(Span::styled(format!(" {} {}{}", it.icon, it.label, suffix), style))
            })
            .collect();
        f.render_widget(Clear, rect);
        f.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            ),
            rect,
        );
    }
}
```

- [ ] **Step 2: Call the renderer in `render()`**

In `src/ui/mod.rs`, in `render` (after the `kill_menu` block at line 46-48) add:

```rust
    if app.kill_menu.is_some() {
        render_kill_menu(f, size, app);
    }
    if app.context_menu.is_some() {
        context_menu::render(f, size, app);
    }
```

- [ ] **Step 3: Show status + `x menu` hint in the footer**

Replace `render_footer` (lines 625-638):

```rust
fn render_footer(f: &mut Frame, area: Rect, app: &App) {
    if let Some(msg) = app.status_text() {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {} ", msg),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
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
        Paragraph::new(Line::from(Span::styled(
            hint,
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
}
```

- [ ] **Step 4: Build**

Run: `cargo build`
Expected: compiles with no errors.

- [ ] **Step 5: Commit**

```bash
git add src/ui/context_menu.rs src/ui/mod.rs
git commit -m "feat(ui): render context-menu cascade + footer status line"
```

---

### Task 8: `main.rs` — wire mouse + keyboard

**Files:**
- Modify: `src/main.rs` — `handle_mouse` (right-click open, left-click-when-open); key loop (context-menu branch + `x`); `expire_status` in the loop.

**Interfaces:**
- Consumes: `App::open_context_menu`, `context_menu_nav/back/select/click`, `selected_tree_index`, `expire_status`.

- [ ] **Step 1: Expire the status message each tick**

In `src/main.rs`, just before the render call (line 162, `if last_render.elapsed() >= ...`) add:

```rust
        app.expire_status();
        if last_render.elapsed() >= Duration::from_millis(100) {
```

- [ ] **Step 2: Add the context-menu key branch**

In the key handling, after the kill-menu block (which ends at line 277, before the drill-down block at line 279) insert:

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
                                app.context_menu_back();
                                continue;
                            }
                            _ => {
                                continue;
                            }
                        }
                    }
```

- [ ] **Step 3: Add the `x` opener in normal mode**

In the normal-mode `match key.code` block (lines 344-392), add an arm (e.g. after the `KeyCode::Char('T')` arm at line 391):

```rust
                        KeyCode::Char('x') => {
                            if app.pane == crate::app::Pane::Sidebar {
                                let search_h: u16 =
                                    if app.search_active || !app.search_query.is_empty() {
                                        1
                                    } else {
                                        0
                                    };
                                let sidebar_list_top = 1 + search_h + 2;
                                if let Some(idx) = app.selected_tree_index() {
                                    let sel = app.selection.clone();
                                    app.open_context_menu(sel, 2, sidebar_list_top + idx as u16);
                                }
                            }
                        }
```

- [ ] **Step 4: Add the right-click and left-click-when-open mouse handling**

In `handle_mouse` (`src/main.rs:454`), at the very start of the `match ev.kind` body, intercept left-clicks while the menu is open. Change the `MouseEventKind::Down(MouseButton::Left)` arm to first handle an open menu:

```rust
        MouseEventKind::Down(MouseButton::Left) => {
            // An open context menu consumes the click: pick an item or close.
            if app.context_menu.is_some() {
                app.context_menu_click(ev.column, ev.row, term_w, term_h);
                return;
            }
            // Close the drill-down modal if the user clicks outside it.
            if app.drilldown_pid.is_some() {
                app.close_drilldown();
                return;
            }
```

Then add a new right-button arm after the `MouseEventKind::Up(MouseButton::Left)` arm (before the final `_ => {}` at line 578):

```rust
        MouseEventKind::Down(MouseButton::Right) => {
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

- [ ] **Step 5: Build and run the test suite**

Run: `cargo build && cargo test`
Expected: compiles; all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs
git commit -m "feat(main): wire right-click + x to open the context menu"
```

---

### Task 9: End-to-end verification + docs

**Files:**
- Modify: `README.md` (footer key list / features — add `x menu` / right-click note).

- [ ] **Step 1: Lint**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: no warnings. Fix any that appear (e.g. unused imports) before continuing.

- [ ] **Step 2: Full test run**

Run: `cargo test`
Expected: all pass.

- [ ] **Step 3: Manual smoke test**

Run: `cargo run`

Verify, in order:
1. Right-click a **repo/cwd project** row → menu shows Collapse, Kill all N…, Focus, Reveal/Open/Copy path.
2. Right-click a **process** row → Inspect, Kill…, Suspend, Resume, Renice ▸, Copy PID/command, Reveal/Open.
3. Right-click `(system)` → only Collapse + Focus (no Kill all).
4. Press `Enter` on **Inspect** → menu closes and the drill-down opens **and responds to `j/k/Esc`** (confirms close-on-activate).
5. Open a process menu, select **Renice ▸** → cascade submenu appears; pick **Low (+10)** → status line shows `reniced <pid> → +10`.
6. **Copy PID** → status line shows `copied pid <n>`; paste elsewhere to confirm.
7. Press `x` on the focused row → menu opens at that row; `j/k` navigate, `Esc` closes.
8. Right-click near the right/bottom edge → popup stays fully on-screen.

- [ ] **Step 4: Update the README**

In `README.md`, add `x` / right-click to the documented key list or features section (match the existing wording style), e.g. a bullet: "Right-click (or `x`) a sidebar row for a context menu: inspect, kill/suspend/renice, copy, reveal/open."

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs: document sidebar context menu (right-click / x)"
```

---

## Self-Review

**Spec coverage:**
- Anchored popup + `x` trigger → Tasks 4, 6, 8. ✓
- Cascade renice submenu → `build_renice` (T1), `context_menu_open_submenu` (T6), render loop over levels (T7). ✓
- Kill… reuses centered picker, retargeted to PID sets → `KillMenu { targets }` (T5), `menu_kill_target` + `open_kill_menu_targets` (T5/T6). ✓
- Context-sensitive content (process / repo-cwd / bundle / system / all) → `build_*` (T1), `open_context_menu` resolution (T6). ✓
- Bundle reveal path from member exe → `app_ancestor` (T5) + `bucket_app_path` (T6). ✓
- Metadata from cached `ProcSample` via `process_by_pid` → used throughout T6 (no fresh `System`). ✓
- Close-on-activate (Inspect vs drill-down) → `outcome_for` + `context_menu_select` (T6), manual check (T9 step 3.4). ✓
- Platform gating → `caps()` (T2), builder `enabled` flags (T1), `#[cfg(unix)]` renice (T2). ✓
- Status line feedback + `x menu` hint → `set_status`/`expire_status`/`status_text` (T5), footer (T7), expiry tick (T8). ✓
- Config `[external]` editor/terminal → T3, mapped in `apply_config`/`to_config_patch` (T6). ✓
- Testing convention (inline `#[cfg(test)]`) → T1-T6. ✓
- Non-goals (no process-table right-click, no hover) → not implemented, as intended. ✓

**Placeholder scan:** No TBD/TODO; every code step shows complete code. ✓

**Type consistency:** `MenuAction`/`Outcome`/`Caps`/`BucketKind` names identical across T1, T6, T7. `KillMenu { targets, name }` consistent across T5 (struct), T5 (`render_kill_menu`), T6 (`menu_kill_target`/`open_kill_menu_targets`). `ExternalCfg { editor: Option<String>, terminal: String }` (actions, T2) vs `ExternalConfig { editor: String, terminal: String }` (config, T3) — distinct types bridged explicitly in `apply_config`/`to_config_patch` (T6). `place`/`MENU_W` defined in T4, consumed in T6 (`context_menu_click`) and T7 (`render`). ✓
