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
