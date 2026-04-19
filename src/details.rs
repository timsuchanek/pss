//! On-demand, per-pid detail loaders for the drill-down modal.
//!
//! These functions shell out to `lsof` / `ps` and read `/proc` where
//! available. They're only called when the user opens a tab, so the cost
//! is bounded and the code stays cross-platform without extra crates.

use std::process::Command;
use std::time::Duration;

const CMD_TIMEOUT: Duration = Duration::from_millis(1500);

#[derive(Clone, Debug)]
pub struct SocketEntry {
    pub proto: String,       // TCP, TCP6, UDP, UDP6, UNIX
    pub local: String,       // addr:port or path
    pub remote: String,      // addr:port or "*"
    pub state: String,       // LISTEN, ESTABLISHED, etc
    pub fd: String,          // numeric fd or "cwd"/"txt"/etc
}

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub fd: String,
    pub kind: String,  // REG, DIR, PIPE, CHR, FIFO, etc
    pub path: String,
    pub size: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct EnvEntry {
    pub key: String,
    pub value: String,
}

#[derive(Clone, Debug, Default)]
pub struct Details {
    pub env: Option<Vec<EnvEntry>>,
    pub files: Option<Vec<FileEntry>>,
    pub sockets: Option<Vec<SocketEntry>>,
    pub threads_count: Option<u32>,
    pub fds_count: Option<u32>,
    pub nice: Option<i32>,
}

pub fn fetch_env(pid: u32) -> Vec<EnvEntry> {
    #[cfg(target_os = "linux")]
    {
        read_linux_environ(pid)
    }
    #[cfg(target_os = "macos")]
    {
        read_mac_env_via_ps(pid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Vec::new()
    }
}

pub fn fetch_files(pid: u32) -> Vec<FileEntry> {
    run_lsof(pid, &["-n", "-P"])
        .into_iter()
        .filter(|f| matches!(f.kind.as_str(), "REG" | "DIR" | "PIPE" | "FIFO" | "CHR" | "BLK" | "LINK"))
        .collect()
}

pub fn fetch_sockets(pid: u32) -> Vec<SocketEntry> {
    // Query TCP + UDP + unix sockets for this pid.
    let lines = run_cmd_stdout(
        "lsof",
        &[
            "-a", "-n", "-P", "-iTCP", "-iUDP", "-iUDP6",
            "-p", &pid.to_string(),
        ],
    );
    let mut out = parse_lsof_sockets(&lines);

    // Append UNIX sockets (lsof without -i filter, then filter type=unix).
    let all = run_lsof(pid, &["-n", "-P"]);
    for f in all {
        if f.kind == "unix" || f.kind == "systm" || f.kind == "PIPE" && f.path.starts_with('/') {
            out.push(SocketEntry {
                proto: f.kind.clone(),
                local: f.path,
                remote: String::new(),
                state: String::new(),
                fd: f.fd,
            });
        }
    }
    out
}

pub fn fetch_fd_and_nice(pid: u32) -> (Option<u32>, Option<i32>) {
    #[cfg(target_os = "linux")]
    {
        let fds = std::fs::read_dir(format!("/proc/{}/fd", pid))
            .ok()
            .map(|rd| rd.count() as u32);
        let nice = read_linux_nice(pid);
        (fds, nice)
    }
    #[cfg(target_os = "macos")]
    {
        let fds = run_cmd_stdout("lsof", &["-n", "-P", "-p", &pid.to_string()])
            .lines()
            .skip(1) // header
            .count();
        let nice = run_cmd_stdout("ps", &["-o", "nice=", "-p", &pid.to_string()])
            .trim()
            .parse::<i32>()
            .ok();
        (Some(fds as u32), nice)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        (None, None)
    }
}

pub fn fetch_threads_count(pid: u32) -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_dir(format!("/proc/{}/task", pid))
            .ok()
            .map(|rd| rd.count() as u32)
    }
    #[cfg(target_os = "macos")]
    {
        let out = run_cmd_stdout("ps", &["-M", "-p", &pid.to_string()]);
        let n = out.lines().count().saturating_sub(1);
        Some(n as u32)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

// --- Platform specifics -------------------------------------------------

#[cfg(target_os = "linux")]
fn read_linux_environ(pid: u32) -> Vec<EnvEntry> {
    let Ok(raw) = std::fs::read(format!("/proc/{}/environ", pid)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for chunk in raw.split(|b| *b == 0) {
        if chunk.is_empty() {
            continue;
        }
        let s = String::from_utf8_lossy(chunk);
        if let Some((k, v)) = s.split_once('=') {
            out.push(EnvEntry {
                key: k.to_string(),
                value: v.to_string(),
            });
        }
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}

#[cfg(target_os = "linux")]
fn read_linux_nice(pid: u32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    // stat format: pid (comm) state ppid pgrp session tty_nr tpgid flags ...
    // "nice" is at index 18 (0-based). But comm can contain spaces and parens.
    let close = stat.rfind(')')?;
    let rest = &stat[close + 2..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    fields.get(16).and_then(|s| s.parse::<i32>().ok())
}

#[cfg(target_os = "macos")]
fn read_mac_env_via_ps(pid: u32) -> Vec<EnvEntry> {
    // `ps eww -o args= -p PID` prints "<argv> KEY=VAL KEY=VAL ..." for same-uid procs.
    // Heuristic parse: after we've seen argv0 + args, once we hit tokens that look
    // like KEY=VALUE (uppercase key, contains '='), treat rest as env.
    let out = run_cmd_stdout("ps", &["eww", "-o", "args=", "-p", &pid.to_string()]);
    let line = out.trim_end_matches('\n');
    let mut entries = Vec::new();
    for tok in line.split_ascii_whitespace() {
        if let Some((k, v)) = tok.split_once('=') {
            if looks_like_env_key(k) {
                entries.push(EnvEntry {
                    key: k.to_string(),
                    value: v.to_string(),
                });
            }
        }
    }
    entries.sort_by(|a, b| a.key.cmp(&b.key));
    entries
}

#[cfg(target_os = "macos")]
fn looks_like_env_key(k: &str) -> bool {
    // Env keys on unix: uppercase letters/digits/underscores, must start with letter or _.
    if k.is_empty() {
        return false;
    }
    let mut chars = k.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    if !first.is_ascii_uppercase() && first != '_' {
        return false;
    }
    k.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// --- lsof parsing -------------------------------------------------------

fn run_lsof(pid: u32, extra_args: &[&str]) -> Vec<FileEntry> {
    let mut args: Vec<&str> = Vec::new();
    args.extend_from_slice(extra_args);
    args.push("-p");
    let pid_s = pid.to_string();
    args.push(&pid_s);
    let out = run_cmd_stdout("lsof", &args);
    parse_lsof_files(&out)
}

fn parse_lsof_files(out: &str) -> Vec<FileEntry> {
    let mut entries = Vec::new();
    for line in out.lines().skip(1) {
        // COMMAND  PID  USER  FD  TYPE  DEVICE  SIZE/OFF  NODE  NAME
        let fields: Vec<&str> = line.splitn(9, char::is_whitespace).collect();
        if fields.len() < 9 {
            continue;
        }
        let fd = fields[3].to_string();
        let kind = fields[4].to_string();
        let size = fields[6].parse::<u64>().ok();
        let path = fields[8].to_string();
        entries.push(FileEntry { fd, kind, path, size });
    }
    entries
}

fn parse_lsof_sockets(out: &str) -> Vec<SocketEntry> {
    let mut sockets = Vec::new();
    for line in out.lines().skip(1) {
        let fields: Vec<&str> = line.splitn(10, char::is_whitespace).collect();
        if fields.len() < 9 {
            continue;
        }
        let fd = fields[3].to_string();
        let proto = fields[4].to_string();
        if !(proto == "IPv4" || proto == "IPv6") {
            // lsof -iTCP -iUDP reports TYPE as IPv4/IPv6; protocol is in NODE field.
            continue;
        }
        let node_proto = fields[7].to_string(); // TCP / UDP
        let name = fields[8].to_string();

        // name format: "addr:port" or "addr:port->remote:port (STATE)"
        let mut local = name.clone();
        let mut remote = String::new();
        let mut state = String::new();
        if let Some(arrow) = name.find("->") {
            local = name[..arrow].to_string();
            let rest = &name[arrow + 2..];
            if let Some(paren) = rest.find(" (") {
                remote = rest[..paren].to_string();
                state = rest[paren + 2..]
                    .trim_end_matches(')')
                    .to_string();
            } else {
                remote = rest.to_string();
            }
        } else if let Some(paren) = name.find(" (") {
            local = name[..paren].to_string();
            state = name[paren + 2..].trim_end_matches(')').to_string();
        }

        sockets.push(SocketEntry {
            proto: node_proto,
            local,
            remote,
            state,
            fd,
        });
    }
    sockets
}

fn run_cmd_stdout(cmd: &str, args: &[&str]) -> String {
    let Some(child) = Command::new(cmd)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()
    else {
        return String::new();
    };

    let start = std::time::Instant::now();
    let Ok(output) = wait_with_timeout(child, CMD_TIMEOUT, start) else {
        return String::new();
    };
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
    start: std::time::Instant,
) -> Result<std::process::Output, ()> {
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                return child.wait_with_output().map_err(|_| ());
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    return Err(());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return Err(()),
        }
    }
}
