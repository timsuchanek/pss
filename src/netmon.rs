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

#[cfg(not(target_os = "macos"))]
mod stub {
    use super::NetRates;
    pub struct NetMon;
    impl NetMon {
        pub fn new() -> Option<Self> {
            None
        }
        pub fn sample(&mut self) -> Option<NetRates> {
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
