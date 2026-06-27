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
