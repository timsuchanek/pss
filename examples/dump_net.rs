use pss::netmon::NetMon;
use std::{thread::sleep, time::Duration};

fn main() {
    let Some(mut mon) = NetMon::new() else {
        eprintln!("netmon: init failed");
        std::process::exit(1);
    };
    // Prime sample.
    let _ = mon.sample();
    for _ in 0..5 {
        sleep(Duration::from_secs(1));
        match mon.sample() {
            None => println!("no sample"),
            Some(r) => println!(
                "↑ {:>10.1} KB/s   ↓ {:>10.1} KB/s",
                r.tx_bytes_per_sec / 1024.0,
                r.rx_bytes_per_sec / 1024.0,
            ),
        }
    }
}
