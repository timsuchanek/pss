use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use crate::collector::{Collector, History, ProcSample, Snapshot};
use crate::heuristics;
use crate::llm::{LlmDigest, Recommendation};

const LLM_STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(45);

#[derive(Debug)]
pub enum AppEvent {
    Snapshot(Snapshot),
    RequestRecommendations,
    Recommendations(Vec<Recommendation>),
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
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Cpu => "cpu",
            SortKey::Mem => "mem",
            SortKey::Name => "name",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarSortKey {
    Cpu,
    Mem,
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
}

impl App {
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
        }
    }

    pub fn open_drilldown(&mut self, pid: u32) {
        self.drilldown_pid = Some(pid);
    }

    pub fn close_drilldown(&mut self) {
        self.drilldown_pid = None;
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
            let key = self.bucket_for(p);
            let entry = map.entry(key.clone()).or_insert_with(|| Bucket {
                key,
                cpu: 0.0,
                mem: 0,
                pids: Vec::new(),
            });
            entry.cpu += p.cpu;
            entry.mem += p.mem;
            entry.pids.push(p.pid);
        }
        let mut buckets: Vec<_> = map.into_values().collect();
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
                .filter(|p| self.pid_visible(p.pid))
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
        // Synthetic "all" root.
        out.push(TreeRow {
            depth: 0,
            label: "▣ all".to_string(),
            has_children: false,
            is_expanded: false,
            cpu: self.buckets.iter().map(|b| b.cpu).sum(),
            mem: self.buckets.iter().map(|b| b.mem).sum(),
            selection: Selection::All,
        });
        let searching = !self.search_query.is_empty();
        for bucket in &self.buckets {
            let label = bucket.key.label();
            if !self.bucket_visible(&label) {
                continue;
            }
            // While searching, force every visible bucket open so matching
            // children are actually visible.
            let expanded = searching || !self.collapsed.contains(&label);
            out.push(TreeRow {
                depth: 0,
                label: label.clone(),
                has_children: true,
                is_expanded: expanded,
                cpu: bucket.cpu,
                mem: bucket.mem,
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
                        out.push(TreeRow {
                            depth: 1,
                            label: format!("{} [{}]", truncate_for_tree(&p.name, 18), p.pid),
                            has_children: false,
                            is_expanded: false,
                            cpu: p.cpu,
                            mem: p.mem,
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
        };
        match dir {
            SortDir::Asc => base,
            SortDir::Desc => base.reverse(),
        }
    });
}

fn kill_pid(pid: u32) {
    use sysinfo::{Pid, ProcessesToUpdate, Signal};
    let mut sys = sysinfo::System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
    if let Some(proc) = sys.process(Pid::from_u32(pid)) {
        proc.kill_with(Signal::Term);
    }
}
