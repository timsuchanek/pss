use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use crate::collector::{Collector, History, ProcSample, Snapshot};
use crate::details::{self, Details};
use crate::heuristics;
use crate::llm::{LlmDigest, Recommendation};
use crate::netmon::{NetRates, PerPidRates};
use crate::thermal::ThermalSnapshot;

const LLM_STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(45);

#[derive(Debug)]
pub enum AppEvent {
    Snapshot(Snapshot),
    RequestRecommendations,
    Recommendations(Vec<Recommendation>),
    Thermal(ThermalSnapshot),
    Net(NetRates),
    PerPidNet(PerPidRates),
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub enum BucketKey {
    Repo(PathBuf),
    Cwd(PathBuf),
    Bundle(String),
    System,
    Unknown,
}

impl BucketKey {
    pub fn label(&self) -> String {
        match self {
            BucketKey::Repo(p) | BucketKey::Cwd(p) => display_path(p),
            BucketKey::Bundle(s) => format!("{} (bundle)", s),
            BucketKey::System => "(system)".into(),
            BucketKey::Unknown => "(unknown)".into(),
        }
    }
}

fn display_path(p: &PathBuf) -> String {
    if let Ok(home) = std::env::var("HOME") {
        if let Ok(rel) = p.strip_prefix(&home) {
            return format!("~/{}", rel.display());
        }
    }
    p.display().to_string()
}

#[derive(Debug, Clone)]
pub struct Bucket {
    pub key: BucketKey,
    pub cpu: f32,
    pub mem: u64,
    pub net_rx: u64, // B/s aggregated across bucket.pids
    pub net_tx: u64,
    pub pids: Vec<u32>,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    #[default]
    Sidebar,
    Processes,
    Recommendations,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    #[default]
    Cpu,
    Mem,
    Name,
    Net,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Cpu => "cpu",
            SortKey::Mem => "mem",
            SortKey::Name => "name",
            SortKey::Net => "net",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarSortKey {
    Cpu,
    Mem,
    Net,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

impl SortDir {
    pub fn glyph(self) -> &'static str {
        match self {
            SortDir::Asc => "▲",
            SortDir::Desc => "▼",
        }
    }
    pub fn flipped(self) -> Self {
        match self {
            SortDir::Asc => SortDir::Desc,
            SortDir::Desc => SortDir::Asc,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecsSource {
    Local,
    Llm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    All,
    Bucket(String),            // bucket label
    Process(String, u32),      // (bucket label, pid)
}

impl Default for Selection {
    fn default() -> Self {
        Selection::All
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeTarget {
    SidebarWidth,
    ChartHeight,
    RecsHeight,
}

pub struct App {
    pub collector: Collector,
    pub history: History,
    pub buckets: Vec<Bucket>,
    pub selection: Selection,
    pub collapsed: HashSet<String>,    // bucket labels the user has explicitly collapsed
    pub pane: Pane,
    pub recommendations: Vec<Recommendation>,
    pub recs_source: RecsSource,
    pub llm_received_at: Option<Instant>,
    pub has_llm: bool,
    pub last_digest_hash: u64,
    pub selected_rec: usize,
    pub sort: SortKey,
    pub sidebar_sort_key: SidebarSortKey,
    pub sidebar_sort_dir: SortDir,
    pub show_help: bool,
    // resizable layout
    pub sidebar_width: u16,
    pub chart_height: u16,
    pub recs_height: u16,
    pub resize_target: Option<ResizeTarget>,
    // search / fuzzy filter
    pub search_active: bool,
    pub search_query: String,
    pub fuzzy_pids: Option<HashSet<u32>>,
    pub fuzzy_bucket_labels: Option<HashSet<String>>,
    // drill-down modal
    pub drilldown_pid: Option<u32>,
    pub drilldown_tab: DrilldownTab,
    pub drilldown_details: Option<Details>,
    pub drilldown_scroll: u16,
    // filter toggles
    pub hide_kernel: bool,
    pub only_my_uid: bool,
    pub hide_self: bool,
    pub my_uid: Option<u32>,
    pub self_pid: u32,
    // sampling controls — shared with the sampler task via atomics
    pub sampler: Arc<SamplerCtl>,
    // kill signal menu
    pub kill_menu: Option<KillMenu>,
    // transient status-line message (text, set-at instant)
    pub status_msg: Option<(String, Instant)>,
    // anchored context menu (None when closed)
    pub context_menu: Option<crate::menu::ContextMenu>,
    // editor/terminal config for external open actions
    pub external: crate::actions::ExternalCfg,
    // last-rendered terminal size (cols, rows); updated each render
    pub term_size: (u16, u16),
    // native thermal sensors (macOS IOHID); empty on other platforms
    pub thermal: Option<ThermalSnapshot>,
    pub show_thermal: bool,
    pub thermal_scroll: u16,
    // aggregate network rates (macOS getifaddrs)
    pub net: Option<NetRates>,
    // per-PID network rates (macOS nettop stream), pid -> (rx_bps, tx_bps)
    pub per_pid_net: PerPidRates,
}

#[derive(Clone, Debug)]
pub struct KillMenu {
    pub targets: Vec<u32>,
    pub name: String,
}

#[derive(Debug)]
pub struct SamplerCtl {
    pub paused: AtomicBool,
    pub interval_ms: AtomicU64,
}

impl SamplerCtl {
    pub fn new(interval_ms: u64) -> Self {
        Self {
            paused: AtomicBool::new(false),
            interval_ms: AtomicU64::new(interval_ms),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrilldownTab {
    Facts,
    Env,
    Files,
    Sockets,
    Tree,
}

impl DrilldownTab {
    pub fn label(self) -> &'static str {
        match self {
            DrilldownTab::Facts => "facts",
            DrilldownTab::Env => "env",
            DrilldownTab::Files => "files",
            DrilldownTab::Sockets => "sockets",
            DrilldownTab::Tree => "tree",
        }
    }
    pub const ALL: [DrilldownTab; 5] = [
        DrilldownTab::Facts,
        DrilldownTab::Env,
        DrilldownTab::Files,
        DrilldownTab::Sockets,
        DrilldownTab::Tree,
    ];
}

impl App {
    pub fn apply_config(&mut self, cfg: &crate::config::Config) {
        self.sidebar_width = cfg.ui.sidebar_width;
        self.chart_height = cfg.ui.chart_height;
        self.recs_height = cfg.ui.recs_height;

        self.sidebar_sort_key = match cfg.sort.sidebar_key.as_str() {
            "mem" => SidebarSortKey::Mem,
            "net" => SidebarSortKey::Net,
            _ => SidebarSortKey::Cpu,
        };
        self.sidebar_sort_dir = match cfg.sort.sidebar_dir.as_str() {
            "asc" => SortDir::Asc,
            _ => SortDir::Desc,
        };
        self.sort = match cfg.sort.processes.as_str() {
            "mem" => SortKey::Mem,
            "name" => SortKey::Name,
            "net" => SortKey::Net,
            _ => SortKey::Cpu,
        };

        self.hide_kernel = cfg.filters.hide_kernel;
        self.only_my_uid = cfg.filters.only_my_uid;
        self.hide_self = cfg.filters.hide_self;

        self.sampler
            .interval_ms
            .store(cfg.sampling.interval_ms.max(100), Ordering::Relaxed);

        self.collapsed = cfg.state.collapsed.iter().cloned().collect();
        self.external = crate::actions::ExternalCfg {
            editor: if cfg.external.editor.is_empty() {
                None
            } else {
                Some(cfg.external.editor.clone())
            },
            terminal: cfg.external.terminal.clone(),
        };
    }

    pub fn to_config_patch(&self, base: &crate::config::Config) -> crate::config::Config {
        use crate::config::*;
        let mut out = base.clone();
        out.ui = UiConfig {
            sidebar_width: self.sidebar_width,
            chart_height: self.chart_height,
            recs_height: self.recs_height,
        };
        out.sort = SortConfig {
            sidebar_key: match self.sidebar_sort_key {
                SidebarSortKey::Cpu => "cpu".into(),
                SidebarSortKey::Mem => "mem".into(),
                SidebarSortKey::Net => "net".into(),
            },
            sidebar_dir: match self.sidebar_sort_dir {
                SortDir::Asc => "asc".into(),
                SortDir::Desc => "desc".into(),
            },
            processes: match self.sort {
                SortKey::Cpu => "cpu".into(),
                SortKey::Mem => "mem".into(),
                SortKey::Name => "name".into(),
                SortKey::Net => "net".into(),
            },
        };
        out.filters = FiltersConfig {
            hide_kernel: self.hide_kernel,
            only_my_uid: self.only_my_uid,
            hide_self: self.hide_self,
        };
        out.sampling = SamplingConfig {
            interval_ms: self.sample_interval_ms(),
        };
        let mut collapsed: Vec<String> = self.collapsed.iter().cloned().collect();
        collapsed.sort();
        out.state = StateConfig { collapsed };
        out.external = crate::config::ExternalConfig {
            editor: self.external.editor.clone().unwrap_or_default(),
            terminal: self.external.terminal.clone(),
        };
        out
    }

    pub fn new() -> Self {
        Self {
            collector: Collector::new(),
            history: History::new(),
            buckets: Vec::new(),
            selection: Selection::All,
            collapsed: HashSet::new(),
            pane: Pane::Sidebar,
            recommendations: Vec::new(),
            recs_source: RecsSource::Local,
            llm_received_at: None,
            has_llm: false,
            last_digest_hash: 0,
            selected_rec: 0,
            sort: SortKey::default(),
            sidebar_sort_key: SidebarSortKey::Cpu,
            sidebar_sort_dir: SortDir::Desc,
            show_help: false,
            sidebar_width: 54,
            chart_height: 18,
            recs_height: 8,
            resize_target: None,
            search_active: false,
            search_query: String::new(),
            fuzzy_pids: None,
            fuzzy_bucket_labels: None,
            drilldown_pid: None,
            drilldown_tab: DrilldownTab::Facts,
            drilldown_details: None,
            drilldown_scroll: 0,
            hide_kernel: true,
            only_my_uid: false,
            hide_self: true,
            my_uid: current_uid(),
            self_pid: std::process::id(),
            sampler: Arc::new(SamplerCtl::new(1000)),
            kill_menu: None,
            status_msg: None,
            context_menu: None,
            external: crate::actions::ExternalCfg::default(),
            term_size: (80, 24),
            thermal: None,
            show_thermal: false,
            thermal_scroll: 0,
            net: None,
            per_pid_net: PerPidRates::default(),
        }
    }

    pub fn set_thermal(&mut self, t: ThermalSnapshot) {
        self.thermal = Some(t);
    }

    pub fn set_net(&mut self, r: NetRates) {
        self.net = Some(r);
    }

    pub fn set_per_pid_net(&mut self, r: PerPidRates) {
        self.per_pid_net = r;
        // Re-compute per-bucket net and re-sort so sidebar reflects the
        // fresh rates even between process snapshots.
        for b in self.buckets.iter_mut() {
            let (rx, tx) = b
                .pids
                .iter()
                .filter_map(|pid| self.per_pid_net.rates.get(pid))
                .fold((0u64, 0u64), |acc, (r, t)| (acc.0 + r, acc.1 + t));
            b.net_rx = rx;
            b.net_tx = tx;
        }
        sort_buckets(&mut self.buckets, self.sidebar_sort_key, self.sidebar_sort_dir);
    }

    pub fn pid_net_rate(&self, pid: u32) -> Option<(u64, u64)> {
        self.per_pid_net.rates.get(&pid).copied()
    }

    pub fn toggle_thermal_overlay(&mut self) {
        self.show_thermal = !self.show_thermal;
        if self.show_thermal {
            self.thermal_scroll = 0;
        }
    }

    pub fn thermal_scroll_by(&mut self, delta: i32) {
        let new = (self.thermal_scroll as i32 + delta).max(0) as u16;
        self.thermal_scroll = new;
    }

    pub fn is_paused(&self) -> bool {
        self.sampler.paused.load(Ordering::Relaxed)
    }
    pub fn sample_interval_ms(&self) -> u64 {
        self.sampler.interval_ms.load(Ordering::Relaxed)
    }

    pub fn toggle_hide_kernel(&mut self) {
        self.hide_kernel = !self.hide_kernel;
    }
    pub fn toggle_only_my_uid(&mut self) {
        self.only_my_uid = !self.only_my_uid;
    }
    pub fn toggle_hide_self(&mut self) {
        self.hide_self = !self.hide_self;
    }

    pub fn toggle_paused(&mut self) {
        let cur = self.sampler.paused.load(Ordering::Relaxed);
        self.sampler.paused.store(!cur, Ordering::Relaxed);
    }
    pub fn sampling_faster(&mut self) {
        let cur = self.sampler.interval_ms.load(Ordering::Relaxed);
        let next = cur.saturating_sub(250).max(250);
        self.sampler.interval_ms.store(next, Ordering::Relaxed);
    }
    pub fn sampling_slower(&mut self) {
        let cur = self.sampler.interval_ms.load(Ordering::Relaxed);
        let next = (cur + 250).min(5000);
        self.sampler.interval_ms.store(next, Ordering::Relaxed);
    }

    pub fn open_kill_menu(&mut self) {
        let (pid, name) = match self.pane {
            Pane::Processes | Pane::Sidebar => {
                if let Some(pid) = self.selected_pid() {
                    let name = self
                        .process_by_pid(pid)
                        .map(|p| p.name.clone())
                        .unwrap_or_default();
                    (Some(pid), name)
                } else {
                    (None, String::new())
                }
            }
            Pane::Recommendations => {
                let rec = self.recommendations.get(self.selected_rec);
                let pid = rec.and_then(|r| r.pid);
                let name = rec.map(|r| r.target.clone()).unwrap_or_default();
                (pid, name)
            }
        };
        // In the drill-down, prefer the drill-down target.
        let (pid, name) = if let Some(dp) = self.drilldown_pid {
            let name = self
                .process_by_pid(dp)
                .map(|p| p.name.clone())
                .unwrap_or(name);
            (Some(dp), name)
        } else {
            (pid, name)
        };
        if let Some(pid) = pid {
            self.kill_menu = Some(KillMenu { targets: vec![pid], name });
        }
    }

    pub fn open_kill_menu_targets(&mut self, targets: Vec<u32>, name: String) {
        if targets.is_empty() {
            return;
        }
        self.kill_menu = Some(KillMenu { targets, name });
    }

    pub fn close_kill_menu(&mut self) {
        self.kill_menu = None;
    }

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

    pub fn proc_passes_filters(&self, p: &ProcSample) -> bool {
        if self.hide_self && p.pid == self.self_pid {
            return false;
        }
        if self.only_my_uid {
            if let (Some(mine), Some(his)) = (self.my_uid, p.uid) {
                if mine != his {
                    return false;
                }
            }
        }
        if self.hide_kernel && is_kernelish(p) {
            return false;
        }
        true
    }

    pub fn open_drilldown(&mut self, pid: u32) {
        let changed = self.drilldown_pid != Some(pid);
        self.drilldown_pid = Some(pid);
        self.drilldown_scroll = 0;
        if changed {
            self.drilldown_tab = DrilldownTab::Facts;
            self.drilldown_details = Some(Details::default());
        }
    }

    pub fn close_drilldown(&mut self) {
        self.drilldown_pid = None;
        self.drilldown_details = None;
        self.drilldown_scroll = 0;
    }

    pub fn drilldown_set_tab(&mut self, tab: DrilldownTab) {
        self.drilldown_tab = tab;
        self.drilldown_scroll = 0;
        self.ensure_drilldown_loaded_for_tab();
    }

    pub fn drilldown_next_tab(&mut self) {
        let cur = DrilldownTab::ALL
            .iter()
            .position(|t| *t == self.drilldown_tab)
            .unwrap_or(0);
        let next = (cur + 1) % DrilldownTab::ALL.len();
        self.drilldown_set_tab(DrilldownTab::ALL[next]);
    }

    pub fn drilldown_prev_tab(&mut self) {
        let cur = DrilldownTab::ALL
            .iter()
            .position(|t| *t == self.drilldown_tab)
            .unwrap_or(0);
        let next = (cur + DrilldownTab::ALL.len() - 1) % DrilldownTab::ALL.len();
        self.drilldown_set_tab(DrilldownTab::ALL[next]);
    }

    pub fn drilldown_scroll_by(&mut self, delta: i32) {
        let cur = self.drilldown_scroll as i32;
        let next = (cur + delta).max(0) as u16;
        self.drilldown_scroll = next;
    }

    /// Fetches data for the active drill-down tab if not already loaded.
    pub fn ensure_drilldown_loaded_for_tab(&mut self) {
        let Some(pid) = self.drilldown_pid else {
            return;
        };
        let mut d = self.drilldown_details.take().unwrap_or_default();
        match self.drilldown_tab {
            DrilldownTab::Facts => {
                if d.fds_count.is_none() || d.nice.is_none() {
                    let (fds, nice) = details::fetch_fd_and_nice(pid);
                    d.fds_count = fds;
                    d.nice = nice;
                }
                if d.threads_count.is_none() {
                    d.threads_count = details::fetch_threads_count(pid);
                }
            }
            DrilldownTab::Env => {
                if d.env.is_none() {
                    d.env = Some(details::fetch_env(pid));
                }
            }
            DrilldownTab::Files => {
                if d.files.is_none() {
                    d.files = Some(details::fetch_files(pid));
                }
            }
            DrilldownTab::Sockets => {
                if d.sockets.is_none() {
                    d.sockets = Some(details::fetch_sockets(pid));
                }
            }
            DrilldownTab::Tree => {
                // computed inline at render time from app state
            }
        }
        self.drilldown_details = Some(d);
    }

    pub fn refresh_drilldown(&mut self) {
        // Clear the cache for the current pid so the active tab re-fetches.
        self.drilldown_details = Some(Details::default());
        self.drilldown_scroll = 0;
        self.ensure_drilldown_loaded_for_tab();
    }

    /// Ancestors of `pid` walking up the ppid chain.
    pub fn ancestors_of(&self, pid: u32) -> Vec<&ProcSample> {
        let Some(latest) = self.history.latest() else {
            return Vec::new();
        };
        let mut chain = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cur = latest.procs.iter().find(|p| p.pid == pid).and_then(|p| p.ppid);
        while let Some(ppid) = cur {
            if !seen.insert(ppid) {
                break;
            }
            if let Some(parent) = latest.procs.iter().find(|p| p.pid == ppid) {
                chain.push(parent);
                cur = parent.ppid;
            } else {
                break;
            }
        }
        chain
    }

    pub fn drilldown_proc(&self) -> Option<&ProcSample> {
        let pid = self.drilldown_pid?;
        self.history.latest()?.procs.iter().find(|p| p.pid == pid)
    }

    pub fn process_by_pid(&self, pid: u32) -> Option<&ProcSample> {
        self.history.latest()?.procs.iter().find(|p| p.pid == pid)
    }

    /// CPU sparkline sample values (most recent first is the tail).
    pub fn pid_cpu_history(&self, pid: u32, width: usize) -> Vec<f32> {
        self.metric_history(pid, width, |p| p.cpu)
    }

    /// Memory (RSS bytes → MB as f32).
    pub fn pid_mem_history(&self, pid: u32, width: usize) -> Vec<f32> {
        self.metric_history(pid, width, |p| p.mem as f32 / 1024.0 / 1024.0)
    }

    fn metric_history<F>(&self, pid: u32, width: usize, f: F) -> Vec<f32>
    where
        F: Fn(&ProcSample) -> f32,
    {
        let buf = &self.history.buf;
        let take_n = width.min(buf.len());
        let skip = buf.len() - take_n;
        let mut v = Vec::with_capacity(take_n);
        for snap in buf.iter().skip(skip) {
            let val = snap
                .procs
                .iter()
                .find(|p| p.pid == pid)
                .map(&f)
                .unwrap_or(0.0);
            v.push(val);
        }
        v
    }

    pub fn children_of(&self, pid: u32) -> Vec<&ProcSample> {
        let Some(latest) = self.history.latest() else {
            return Vec::new();
        };
        let mut v: Vec<&ProcSample> = latest
            .procs
            .iter()
            .filter(|p| p.ppid == Some(pid))
            .collect();
        v.sort_by(|a, b| {
            b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal)
        });
        v
    }

    // --- Search ---
    pub fn enter_search(&mut self) {
        self.search_active = true;
        // keep any prior query so '/' toggles back to the last search
        self.recompute_search();
    }

    pub fn exit_search_commit(&mut self) {
        self.search_active = false;
        // filter stays applied
    }

    pub fn exit_search_cancel(&mut self) {
        self.search_active = false;
        self.search_query.clear();
        self.fuzzy_pids = None;
        self.fuzzy_bucket_labels = None;
    }

    pub fn search_push(&mut self, c: char) {
        self.search_query.push(c);
        self.recompute_search();
    }

    pub fn search_pop(&mut self) {
        self.search_query.pop();
        self.recompute_search();
    }

    pub fn recompute_search(&mut self) {
        if self.search_query.is_empty() {
            self.fuzzy_pids = None;
            self.fuzzy_bucket_labels = None;
            return;
        }
        use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
        use nucleo_matcher::{Config, Matcher, Utf32Str};

        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse(
            &self.search_query,
            CaseMatching::Smart,
            Normalization::Smart,
        );

        let mut pids: HashSet<u32> = HashSet::new();
        if let Some(latest) = self.history.latest() {
            for p in &latest.procs {
                // Match only the process name — i.e. the text visible in the
                // sidebar row. Matching against cmd/cwd made fuzzy hits fire
                // on unrelated processes whose paths happened to contain the
                // query's letters.
                let mut buf = Vec::new();
                if pattern
                    .score(Utf32Str::new(&p.name, &mut buf), &mut matcher)
                    .is_some()
                {
                    pids.insert(p.pid);
                }
            }
        }

        let mut labels: HashSet<String> = HashSet::new();
        for b in &self.buckets {
            let label = b.key.label();
            let mut buf = Vec::new();
            let matches_label = pattern
                .score(Utf32Str::new(&label, &mut buf), &mut matcher)
                .is_some();
            let has_matching_proc = b.pids.iter().any(|pid| pids.contains(pid));
            if matches_label || has_matching_proc {
                labels.insert(label);
            }
        }

        self.fuzzy_pids = Some(pids);
        self.fuzzy_bucket_labels = Some(labels);
    }

    pub fn pid_visible(&self, pid: u32) -> bool {
        match &self.fuzzy_pids {
            None => true,
            Some(set) => set.contains(&pid),
        }
    }

    pub fn bucket_visible(&self, label: &str) -> bool {
        match &self.fuzzy_bucket_labels {
            None => true,
            Some(set) => set.contains(label),
        }
    }

    pub fn set_sort(&mut self, s: SortKey) {
        self.sort = s;
    }

    pub fn sidebar_toggle_sort(&mut self, key: SidebarSortKey) {
        if self.sidebar_sort_key == key {
            self.sidebar_sort_dir = self.sidebar_sort_dir.flipped();
        } else {
            self.sidebar_sort_key = key;
            self.sidebar_sort_dir = SortDir::Desc;
        }
        sort_buckets(&mut self.buckets, self.sidebar_sort_key, self.sidebar_sort_dir);
    }

    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    pub fn push_snapshot(&mut self, snap: Snapshot) {
        self.rebucket(&snap);
        self.history.push(snap);
        self.refresh_local_recs();
        if self.search_query.is_empty() {
            self.fuzzy_pids = None;
            self.fuzzy_bucket_labels = None;
        } else {
            self.recompute_search();
        }
    }

    fn refresh_local_recs(&mut self) {
        let llm_fresh = self
            .llm_received_at
            .map(|t| t.elapsed() < LLM_STALE_AFTER)
            .unwrap_or(false);
        if llm_fresh {
            return;
        }
        self.recommendations = heuristics::generate(self);
        self.recs_source = RecsSource::Local;
    }

    fn rebucket(&mut self, snap: &Snapshot) {
        let mut map: BTreeMap<BucketKey, Bucket> = BTreeMap::new();
        for p in &snap.procs {
            if !self.proc_passes_filters(p) {
                continue;
            }
            let key = self.bucket_for(p);
            let entry = map.entry(key.clone()).or_insert_with(|| Bucket {
                key,
                cpu: 0.0,
                mem: 0,
                net_rx: 0,
                net_tx: 0,
                pids: Vec::new(),
            });
            entry.cpu += p.cpu;
            entry.mem += p.mem;
            entry.pids.push(p.pid);
        }
        let mut buckets: Vec<_> = map.into_values().collect();
        for b in buckets.iter_mut() {
            let (rx, tx) = b
                .pids
                .iter()
                .filter_map(|pid| self.per_pid_net.rates.get(pid))
                .fold((0u64, 0u64), |acc, (r, t)| (acc.0 + r, acc.1 + t));
            b.net_rx = rx;
            b.net_tx = tx;
        }
        sort_buckets(&mut buckets, self.sidebar_sort_key, self.sidebar_sort_dir);
        self.buckets = buckets;
        // if selection references a bucket that vanished, fall back to All
        if let Selection::Bucket(label) | Selection::Process(label, _) = &self.selection.clone() {
            if !self.buckets.iter().any(|b| b.key.label() == *label) {
                self.selection = Selection::All;
            }
        }
    }

    fn bucket_for(&mut self, p: &ProcSample) -> BucketKey {
        if p.ppid.is_none() || p.pid <= 1 {
            return BucketKey::System;
        }
        let Some(cwd) = p.cwd.as_ref() else {
            return BucketKey::Unknown;
        };
        // root path → system
        if cwd.as_os_str() == "/" {
            // fall back to bundle by exe parent
            if let Some(exe) = p.exe.as_ref() {
                if let Some(parent) = exe.parent() {
                    let s = parent.display().to_string();
                    if s.contains(".app/") || s.contains("Program Files") {
                        return BucketKey::Bundle(
                            parent
                                .components()
                                .rev()
                                .find(|c| {
                                    c.as_os_str()
                                        .to_string_lossy()
                                        .ends_with(".app")
                                })
                                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                                .unwrap_or_else(|| "app".into()),
                        );
                    }
                }
            }
            return BucketKey::System;
        }
        if let Some(root) = self.collector.repo_root(cwd) {
            BucketKey::Repo(root)
        } else {
            BucketKey::Cwd(cwd.clone())
        }
    }

    pub fn selected_bucket_ref(&self) -> Option<&Bucket> {
        match &self.selection {
            Selection::All => None,
            Selection::Bucket(label) | Selection::Process(label, _) => {
                self.buckets.iter().find(|b| b.key.label() == *label)
            }
        }
    }

    pub fn selected_pid(&self) -> Option<u32> {
        if let Selection::Process(_, pid) = &self.selection {
            Some(*pid)
        } else {
            None
        }
    }

    pub fn procs_in_selected(&self) -> Vec<&ProcSample> {
        let Some(latest) = self.history.latest() else {
            return Vec::new();
        };
        let mut v: Vec<&ProcSample> = match &self.selection {
            Selection::All => latest
                .procs
                .iter()
                .filter(|p| self.proc_passes_filters(p) && self.pid_visible(p.pid))
                .collect(),
            Selection::Bucket(label) | Selection::Process(label, _) => {
                let Some(bucket) = self.buckets.iter().find(|b| b.key.label() == *label) else {
                    return Vec::new();
                };
                let pids: std::collections::HashSet<u32> =
                    bucket.pids.iter().copied().collect();
                latest
                    .procs
                    .iter()
                    .filter(|p| pids.contains(&p.pid) && self.pid_visible(p.pid))
                    .collect()
            }
        };
        match self.sort {
            SortKey::Cpu => v.sort_by(|a, b| {
                b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal)
            }),
            SortKey::Mem => v.sort_by(|a, b| b.mem.cmp(&a.mem)),
            SortKey::Name => v.sort_by(|a, b| a.name.cmp(&b.name)),
            SortKey::Net => v.sort_by(|a, b| {
                let pr_a = self.pid_net_rate(a.pid).unwrap_or((0, 0));
                let pr_b = self.pid_net_rate(b.pid).unwrap_or((0, 0));
                (pr_b.0 + pr_b.1).cmp(&(pr_a.0 + pr_a.1))
            }),
        }
        v
    }

    pub fn nav_down(&mut self) {
        match self.pane {
            Pane::Sidebar => self.tree_move(1),
            Pane::Processes => {
                let procs = self.procs_in_selected();
                if procs.is_empty() {
                    return;
                }
                let idx = self
                    .selected_pid()
                    .and_then(|pid| procs.iter().position(|p| p.pid == pid))
                    .unwrap_or(0);
                let next = (idx + 1).min(procs.len() - 1);
                if let (Some(p), Some(bucket)) = (procs.get(next), self.selected_bucket_ref()) {
                    self.selection = Selection::Process(bucket.key.label(), p.pid);
                }
            }
            Pane::Recommendations => {
                if self.selected_rec + 1 < self.recommendations.len() {
                    self.selected_rec += 1;
                }
            }
        }
    }

    pub fn nav_up(&mut self) {
        match self.pane {
            Pane::Sidebar => self.tree_move(-1),
            Pane::Processes => {
                let procs = self.procs_in_selected();
                if procs.is_empty() {
                    return;
                }
                let idx = self
                    .selected_pid()
                    .and_then(|pid| procs.iter().position(|p| p.pid == pid))
                    .unwrap_or(0);
                let next = idx.saturating_sub(1);
                if let (Some(p), Some(bucket)) = (procs.get(next), self.selected_bucket_ref()) {
                    self.selection = Selection::Process(bucket.key.label(), p.pid);
                }
            }
            Pane::Recommendations => {
                self.selected_rec = self.selected_rec.saturating_sub(1);
            }
        }
    }

    pub fn expand(&mut self) {
        match self.pane {
            Pane::Sidebar => {
                // If the current selection is a bucket that's collapsed, re-open it.
                // Otherwise move focus to the processes pane.
                if let Selection::Bucket(label) = &self.selection.clone() {
                    if self.collapsed.contains(label) {
                        self.collapsed.remove(label);
                        return;
                    }
                }
                self.pane = Pane::Processes;
            }
            _ => {}
        }
    }

    pub fn collapse(&mut self) {
        match self.pane {
            Pane::Sidebar => {
                // If on a process, jump up to its bucket.
                // If on an open bucket, collapse it.
                match self.selection.clone() {
                    Selection::Process(label, _) => {
                        self.selection = Selection::Bucket(label);
                    }
                    Selection::Bucket(label) => {
                        self.collapsed.insert(label);
                    }
                    Selection::All => {}
                }
            }
            _ => self.pane = Pane::Sidebar,
        }
    }

    /// Walk the visible tree rows by `delta` positions and update selection.
    fn tree_move(&mut self, delta: i32) {
        let rows = self.tree_rows();
        if rows.is_empty() {
            return;
        }
        let cur = rows
            .iter()
            .position(|r| r.selection == self.selection)
            .unwrap_or(0) as i32;
        let next = (cur + delta).clamp(0, rows.len() as i32 - 1) as usize;
        self.selection = rows[next].selection.clone();
    }

    /// Flat, ordered list of visible tree rows (what the sidebar renders).
    pub fn tree_rows(&self) -> Vec<TreeRow> {
        let mut out = Vec::new();
        let (total_rx, total_tx) = self
            .per_pid_net
            .rates
            .values()
            .fold((0u64, 0u64), |acc, (r, t)| (acc.0 + r, acc.1 + t));
        // Synthetic "all" root.
        out.push(TreeRow {
            depth: 0,
            label: "▣ all".to_string(),
            has_children: false,
            is_expanded: false,
            cpu: self.buckets.iter().map(|b| b.cpu).sum(),
            mem: self.buckets.iter().map(|b| b.mem).sum(),
            net_rx: total_rx,
            net_tx: total_tx,
            selection: Selection::All,
        });
        let searching = !self.search_query.is_empty();
        for bucket in &self.buckets {
            let label = bucket.key.label();
            if !self.bucket_visible(&label) {
                continue;
            }
            let expanded = searching || !self.collapsed.contains(&label);
            out.push(TreeRow {
                depth: 0,
                label: label.clone(),
                has_children: true,
                is_expanded: expanded,
                cpu: bucket.cpu,
                mem: bucket.mem,
                net_rx: bucket.net_rx,
                net_tx: bucket.net_tx,
                selection: Selection::Bucket(label.clone()),
            });
            if expanded {
                if let Some(latest) = self.history.latest() {
                    let pids: std::collections::HashSet<u32> =
                        bucket.pids.iter().copied().collect();
                    let mut procs: Vec<&ProcSample> = latest
                        .procs
                        .iter()
                        .filter(|p| pids.contains(&p.pid) && self.pid_visible(p.pid))
                        .collect();
                    procs.sort_by(|a, b| {
                        b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    let limit = if self.search_query.is_empty() { 12 } else { 50 };
                    for p in procs.into_iter().take(limit) {
                        let (rx, tx) = self.per_pid_net.rates.get(&p.pid).copied().unwrap_or((0, 0));
                        out.push(TreeRow {
                            depth: 1,
                            label: format!("{} [{}]", truncate_for_tree(&p.name, 18), p.pid),
                            has_children: false,
                            is_expanded: false,
                            cpu: p.cpu,
                            mem: p.mem,
                            net_rx: rx,
                            net_tx: tx,
                            selection: Selection::Process(label.clone(), p.pid),
                        });
                    }
                }
            }
        }
        out
    }

    // --- Resize helpers ---
    pub fn begin_resize(&mut self, t: ResizeTarget) {
        self.resize_target = Some(t);
    }
    pub fn end_resize(&mut self) {
        self.resize_target = None;
    }
    pub fn drag(&mut self, col: u16, row: u16, term_w: u16, term_h: u16) {
        match self.resize_target {
            Some(ResizeTarget::SidebarWidth) => {
                self.sidebar_width = col.clamp(16, term_w.saturating_sub(40));
            }
            Some(ResizeTarget::ChartHeight) => {
                // chart top = header(1). new chart height = row - 1 (account for header)
                let header = 1u16;
                self.chart_height = row.saturating_sub(header).clamp(6, term_h.saturating_sub(18));
            }
            Some(ResizeTarget::RecsHeight) => {
                // recs top row; new recs height = term_h - 1(footer) - row
                let footer = 1u16;
                self.recs_height = term_h
                    .saturating_sub(row + footer)
                    .clamp(3, term_h.saturating_sub(20));
            }
            None => {}
        }
    }
}

pub struct TreeRow {
    pub depth: u8,
    pub label: String,
    pub has_children: bool,
    pub is_expanded: bool,
    pub cpu: f32,
    pub mem: u64,
    pub net_rx: u64, // B/s
    pub net_tx: u64,
    pub selection: Selection,
}

fn truncate_for_tree(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept)
}

impl App {
    pub fn cycle_pane(&mut self) {
        self.pane = match self.pane {
            Pane::Sidebar => Pane::Processes,
            Pane::Processes => Pane::Recommendations,
            Pane::Recommendations => Pane::Sidebar,
        };
    }

    pub fn kill_selected(&mut self) {
        let pid = match self.pane {
            Pane::Processes | Pane::Sidebar => self.selected_pid(),
            Pane::Recommendations => self
                .recommendations
                .get(self.selected_rec)
                .and_then(|r| r.pid),
        };
        if let Some(pid) = pid {
            kill_pid(pid);
        }
    }

    pub fn digest_for_llm(&mut self) -> Option<LlmDigest> {
        let latest = self.history.latest()?.clone();
        // cheap hash over bucket label + cpu rounded
        let mut h = 0u64;
        for b in &self.buckets {
            h = h
                .wrapping_mul(1315423911)
                .wrapping_add(b.key.label().len() as u64)
                .wrapping_add(b.cpu.round() as u64);
        }
        if h == self.last_digest_hash {
            return None;
        }
        self.last_digest_hash = h;
        Some(LlmDigest::from_state(&latest, &self.buckets))
    }

    pub fn set_recommendations(&mut self, recs: Vec<Recommendation>) {
        if recs.is_empty() {
            return;
        }
        self.recommendations = recs;
        self.recs_source = RecsSource::Llm;
        self.llm_received_at = Some(Instant::now());
        if self.selected_rec >= self.recommendations.len() {
            self.selected_rec = self.recommendations.len().saturating_sub(1);
        }
    }
}

fn sort_buckets(buckets: &mut [Bucket], key: SidebarSortKey, dir: SortDir) {
    use std::cmp::Ordering;
    buckets.sort_by(|a, b| {
        let base = match key {
            SidebarSortKey::Cpu => a.cpu.partial_cmp(&b.cpu).unwrap_or(Ordering::Equal),
            SidebarSortKey::Mem => a.mem.cmp(&b.mem),
            SidebarSortKey::Net => (a.net_rx + a.net_tx).cmp(&(b.net_rx + b.net_tx)),
        };
        match dir {
            SortDir::Asc => base,
            SortDir::Desc => base.reverse(),
        }
    });
}

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

fn current_uid() -> Option<u32> {
    #[cfg(unix)]
    unsafe {
        Some(libc::getuid() as u32)
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn is_kernelish(p: &ProcSample) -> bool {
    // Heuristic across platforms: kernel / system-ish processes tend to have
    // no cwd, no exe, and (often) uid 0. Keep conservative so regular root
    // processes with real cwds still show.
    p.cwd.is_none() && p.exe.is_none() && p.uid == Some(0)
}

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
