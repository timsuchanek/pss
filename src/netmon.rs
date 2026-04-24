//! Aggregate network byte counters via `getifaddrs`.
//!
//! v1 scope (L1): system-wide rx/tx bytes summed across non-loopback
//! interfaces. No sudo, no shell-out, no entitlements. Per-PID attribution
//! lives in a later scope (L2/L3) and needs libproc FFI.
//!
//! Counters on Darwin's `if_data` are 32-bit and wrap around at 4 GB.
//! We sample at 1 Hz and compute deltas with wrapping arithmetic — at any
//! reasonable link speed we can't wrap more than once per interval.

#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(not(target_os = "macos"))]
pub use stub::*;

#[derive(Clone, Copy, Debug, Default)]
pub struct NetRates {
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
}

/// Per-PID rx/tx rates from a streaming `nettop -d` child.
///
/// Rates are B/s (already a delta-over-interval because nettop is in `-d`
/// delta mode). Only PIDs that had non-zero traffic in the last interval
/// appear in the map; the renderer must treat missing PIDs as idle.
#[derive(Clone, Debug, Default)]
pub struct PerPidRates {
    pub rates: std::collections::HashMap<u32, (u64, u64)>,
}

#[cfg(not(target_os = "macos"))]
mod stub {
    use super::{NetRates, PerPidRates};
    pub struct NetMon;
    impl NetMon {
        pub fn new() -> Option<Self> {
            None
        }
        pub fn sample(&mut self) -> Option<NetRates> {
            None
        }
    }

    pub struct PerPidSampler;
    impl PerPidSampler {
        pub fn spawn<F>(_: F) -> Option<Self>
        where
            F: FnMut(PerPidRates) + Send + 'static,
        {
            None
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::NetRates;
    use libc::{AF_LINK, IFF_LOOPBACK, freeifaddrs, getifaddrs, if_data, ifaddrs};
    use std::time::Instant;

    #[derive(Clone, Copy, Debug)]
    struct Totals {
        rx: u64,
        tx: u64,
        at: Instant,
    }

    pub struct NetMon {
        last: Option<Totals>,
    }

    impl NetMon {
        pub fn new() -> Option<Self> {
            Some(Self { last: None })
        }

        pub fn sample(&mut self) -> Option<NetRates> {
            let now = read_totals()?;
            let rates = match self.last {
                None => NetRates::default(),
                Some(prev) => {
                    let dt = now.at.duration_since(prev.at).as_secs_f64().max(1e-3);
                    // Counters on Darwin wrap at 2^32. Deltas must use wrapping
                    // semantics on the low 32 bits, then lift to u64.
                    let drx = (now.rx as u32).wrapping_sub(prev.rx as u32) as u64;
                    let dtx = (now.tx as u32).wrapping_sub(prev.tx as u32) as u64;
                    NetRates {
                        rx_bytes_per_sec: drx as f64 / dt,
                        tx_bytes_per_sec: dtx as f64 / dt,
                    }
                }
            };
            self.last = Some(now);
            Some(rates)
        }
    }

    use super::PerPidRates;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};

    /// Streams `nettop -d -x` and invokes `on_sample` with per-PID rates
    /// once per second. The child is killed when the handle is dropped.
    pub struct PerPidSampler {
        child: Child,
    }

    impl Drop for PerPidSampler {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    impl PerPidSampler {
        pub fn spawn<F>(mut on_sample: F) -> Option<Self>
        where
            F: FnMut(PerPidRates) + Send + 'static,
        {
            // stdbuf -oL forces line-buffered stdout so we see each sample
            // immediately; without it nettop block-buffers when piped.
            let mut child = Command::new("/usr/bin/stdbuf")
                .args([
                    "-oL",
                    "nettop",
                    "-P",
                    "-x",
                    "-d",
                    "-J",
                    "bytes_in,bytes_out",
                    "-s",
                    "1",
                    "-L",
                    "0",
                    "-n",
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .stdin(Stdio::null())
                .spawn()
                .ok()?;

            let stdout = child.stdout.take()?;
            std::thread::spawn(move || {
                let reader = BufReader::new(stdout);
                let mut current: HashMap<u32, (u64, u64)> = HashMap::new();
                let mut in_first_block = true; // first block is baseline totals, not deltas
                for line in reader.lines().flatten() {
                    if line.starts_with("time,") {
                        // Sample boundary — flush the previous block.
                        if !in_first_block {
                            on_sample(PerPidRates {
                                rates: std::mem::take(&mut current),
                            });
                        } else {
                            current.clear();
                            in_first_block = false;
                        }
                        continue;
                    }
                    if let Some((pid, rx, tx)) = parse_row(&line) {
                        if rx != 0 || tx != 0 {
                            current.insert(pid, (rx, tx));
                        }
                    }
                }
            });
            Some(Self { child })
        }
    }

    /// Parse `time,name.pid,bytes_in,bytes_out,` → `(pid, rx, tx)`.
    fn parse_row(line: &str) -> Option<(u32, u64, u64)> {
        let mut fields = line.splitn(5, ',');
        let _time = fields.next()?;
        let name_pid = fields.next()?;
        let rx: u64 = fields.next()?.parse().ok()?;
        let tx: u64 = fields.next()?.parse().ok()?;
        // Name can contain dots (e.g. "io.tailscale.ip.60262"), PID is after
        // the last dot.
        let pid_str = name_pid.rsplit_once('.')?.1;
        let pid: u32 = pid_str.parse().ok()?;
        Some((pid, rx, tx))
    }

    fn read_totals() -> Option<Totals> {
        let mut list: *mut ifaddrs = std::ptr::null_mut();
        let (mut rx, mut tx) = (0u64, 0u64);
        unsafe {
            if getifaddrs(&mut list) != 0 {
                return None;
            }
            let mut cur = list;
            while !cur.is_null() {
                let ifa = &*cur;
                if !ifa.ifa_addr.is_null() && !ifa.ifa_data.is_null() {
                    let family = (*ifa.ifa_addr).sa_family as i32;
                    if family == AF_LINK {
                        let is_loopback = (ifa.ifa_flags & IFF_LOOPBACK as u32) != 0;
                        if !is_loopback {
                            let d = &*(ifa.ifa_data as *const if_data);
                            rx += d.ifi_ibytes as u64;
                            tx += d.ifi_obytes as u64;
                        }
                    }
                }
                cur = ifa.ifa_next;
            }
            freeifaddrs(list);
        }
        Some(Totals {
            rx,
            tx,
            at: Instant::now(),
        })
    }
}
