use std::collections::HashMap;

use crate::app::App;
use crate::llm::Recommendation;

const DEV_SERVERS: &[&str] = &[
    "vite", "webpack", "esbuild", "parcel", "rollup", "nodemon", "next-server",
    "turbo", "snowpack",
];

pub fn generate(app: &App) -> Vec<Recommendation> {
    let Some(latest) = app.history.latest() else {
        return Vec::new();
    };

    // per-pid running avg cpu over visible history
    let mut cpu_stats: HashMap<u32, (f32, u32)> = HashMap::new();
    for snap in app.history.buf.iter() {
        for p in &snap.procs {
            let e = cpu_stats.entry(p.pid).or_insert((0.0, 0));
            e.0 += p.cpu;
            e.1 += 1;
        }
    }

    let mut recs: Vec<Recommendation> = Vec::new();

    // 1. Idle dev servers
    for p in &latest.procs {
        let name_lc = p.name.to_ascii_lowercase();
        let cmd_lc = p.cmd.to_ascii_lowercase();
        let matched = DEV_SERVERS
            .iter()
            .any(|s| name_lc.contains(s) || cmd_lc.contains(s));
        if !matched {
            continue;
        }
        let (sum, n) = cpu_stats.get(&p.pid).copied().unwrap_or((0.0, 1));
        let avg = sum / n.max(1) as f32;
        let mem_mb = p.mem / 1024 / 1024;
        if avg < 1.0 && mem_mb > 150 && n >= 10 {
            recs.push(Recommendation {
                pid: Some(p.pid),
                action: "kill".into(),
                target: format!("{} [{}]", short_name(&p.name, &p.cmd), p.pid),
                reason: format!("dev server idle ~{}s, holding {}M", n, mem_mb),
                confidence: 85,
                estimated_saved_mb: mem_mb,
            });
        }
    }

    // 2. Big idle memory — top 3 by RSS with near-zero CPU
    let mut by_mem: Vec<_> = latest.procs.iter().collect();
    by_mem.sort_by(|a, b| b.mem.cmp(&a.mem));
    for p in by_mem.iter().take(5) {
        let (sum, n) = cpu_stats.get(&p.pid).copied().unwrap_or((0.0, 1));
        let avg = sum / n.max(1) as f32;
        let mem_mb = p.mem / 1024 / 1024;
        if avg < 0.5 && mem_mb > 1024 && n >= 10 {
            // avoid duplicates with dev-server hits
            if recs.iter().any(|r| r.pid == Some(p.pid)) {
                continue;
            }
            recs.push(Recommendation {
                pid: Some(p.pid),
                action: "info".into(),
                target: format!("{} [{}]", short_name(&p.name, &p.cmd), p.pid),
                reason: format!("idle but holding {}M", mem_mb),
                confidence: 60,
                estimated_saved_mb: mem_mb,
            });
        }
    }

    // 3. Orphaned short-lived tools (ppid=1 for cli tools that shouldn't be)
    const ORPHAN_SUSPECTS: &[&str] = &["rg", "ripgrep", "fd", "find", "grep", "sed", "awk"];
    for p in &latest.procs {
        if p.ppid != Some(1) {
            continue;
        }
        if ORPHAN_SUSPECTS.iter().any(|s| p.name.eq_ignore_ascii_case(s)) {
            if recs.iter().any(|r| r.pid == Some(p.pid)) {
                continue;
            }
            recs.push(Recommendation {
                pid: Some(p.pid),
                action: "kill".into(),
                target: format!("{} [{}]", p.name, p.pid),
                reason: "orphaned tool (parent died)".into(),
                confidence: 75,
                estimated_saved_mb: p.mem / 1024 / 1024,
            });
        }
    }

    // 4. Chrome helper sprawl
    let chrome_count = latest
        .procs
        .iter()
        .filter(|p| {
            let n = p.name.to_ascii_lowercase();
            n.contains("chrome helper") || n.contains("google chrome helper")
        })
        .count();
    if chrome_count >= 20 {
        let chrome_mem_mb: u64 = latest
            .procs
            .iter()
            .filter(|p| p.name.to_ascii_lowercase().contains("chrome"))
            .map(|p| p.mem)
            .sum::<u64>()
            / 1024
            / 1024;
        recs.push(Recommendation {
            pid: None,
            action: "info".into(),
            target: format!("chrome · {} helpers", chrome_count),
            reason: format!("{}M across tab processes; consider closing tabs", chrome_mem_mb),
            confidence: 55,
            estimated_saved_mb: chrome_mem_mb / 2,
        });
    }

    recs.sort_by(|a, b| b.confidence.cmp(&a.confidence));
    recs.truncate(5);
    recs
}

fn short_name(name: &str, cmd: &str) -> String {
    // For node-launched tools, try to pull the dev-server name out of the cmdline.
    let cmd_lc = cmd.to_ascii_lowercase();
    for hint in DEV_SERVERS {
        if cmd_lc.contains(hint) {
            return (*hint).to_string();
        }
    }
    name.to_string()
}
