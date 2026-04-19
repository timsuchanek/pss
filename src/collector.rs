use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

const HISTORY_SECONDS: usize = 120;

#[derive(Clone, Debug)]
pub struct ProcSample {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub name: String,
    pub cmd: String,
    pub cpu: f32,
    pub mem: u64,
    pub virt: u64,
    pub cwd: Option<PathBuf>,
    pub exe: Option<PathBuf>,
    pub started_at: u64,        // unix seconds
    pub run_time_secs: u64,
    pub status: String,
    pub uid: Option<u32>,
    pub io_read: u64,           // total bytes read
    pub io_write: u64,          // total bytes written
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub ts: u64,
    pub procs: Vec<ProcSample>,
    pub total_cpu: f32,
    pub total_mem: u64,
    pub avail_mem: u64,
}

pub struct Collector {
    sys: System,
    repo_cache: HashMap<PathBuf, Option<PathBuf>>,
}

impl Collector {
    pub fn new() -> Self {
        let mut sys = System::new();
        sys.refresh_memory();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::everything(),
        );
        Self {
            sys,
            repo_cache: HashMap::new(),
        }
    }

    pub fn sample(&mut self) -> Snapshot {
        self.sys.refresh_memory();
        self.sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::everything().with_cpu(),
        );

        let mut procs = Vec::with_capacity(self.sys.processes().len());
        let mut total_cpu = 0.0f32;
        for (pid, p) in self.sys.processes() {
            let cpu = p.cpu_usage();
            total_cpu += cpu;
            procs.push(ProcSample {
                pid: pid.as_u32(),
                ppid: p.parent().map(|p| p.as_u32()),
                name: p.name().to_string_lossy().into_owned(),
                cmd: p
                    .cmd()
                    .iter()
                    .map(|s| s.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" "),
                cpu,
                mem: p.memory(),
                virt: p.virtual_memory(),
                cwd: p.cwd().map(Path::to_path_buf),
                exe: p.exe().map(Path::to_path_buf),
                started_at: p.start_time(),
                run_time_secs: p.run_time(),
                status: format!("{:?}", p.status()),
                uid: p.user_id().map(|u| **u),
                io_read: p.disk_usage().total_read_bytes,
                io_write: p.disk_usage().total_written_bytes,
            });
        }

        Snapshot {
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default(),
            procs,
            total_cpu,
            total_mem: self.sys.used_memory(),
            avail_mem: self.sys.total_memory(),
        }
    }

    pub fn repo_root(&mut self, path: &Path) -> Option<PathBuf> {
        if let Some(hit) = self.repo_cache.get(path) {
            return hit.clone();
        }
        let result = gix::discover(path)
            .ok()
            .and_then(|repo| repo.work_dir().map(Path::to_path_buf));
        self.repo_cache.insert(path.to_path_buf(), result.clone());
        result
    }
}

/// Rolling history of snapshots for the chart.
pub struct History {
    pub buf: VecDeque<Snapshot>,
    cap: usize,
}

impl History {
    pub fn new() -> Self {
        Self {
            buf: VecDeque::with_capacity(HISTORY_SECONDS),
            cap: HISTORY_SECONDS,
        }
    }
    pub fn push(&mut self, s: Snapshot) {
        if self.buf.len() == self.cap {
            self.buf.pop_front();
        }
        self.buf.push_back(s);
    }
    pub fn latest(&self) -> Option<&Snapshot> {
        self.buf.back()
    }
}
