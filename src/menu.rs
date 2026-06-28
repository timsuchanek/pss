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
    CopyReason(String),
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

pub fn build_info_rec(caps: Caps, reason: String) -> Vec<MenuItem> {
    vec![item("⧉", "Copy reason", MenuAction::CopyReason(reason), caps.clipboard, false)]
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

    #[test]
    fn info_rec_menu_is_copy_reason_gated_by_clipboard() {
        let m = build_info_rec(ALL, "idle 90s".to_string());
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].action, MenuAction::CopyReason("idle 90s".to_string()));
        assert!(m[0].enabled);
        assert!(!m[0].opens_submenu);
        let m2 = build_info_rec(NO_GUI, "x".to_string());
        assert!(!m2[0].enabled, "Copy reason disabled without clipboard");
    }
}
