use pss::thermal::ThermalReader;

fn main() {
    let Some(reader) = ThermalReader::new() else {
        eprintln!("thermal: IOHID client creation failed");
        std::process::exit(1);
    };
    let sensors = reader.read();
    if sensors.is_empty() {
        eprintln!("thermal: no sensors returned");
        std::process::exit(2);
    }
    println!("{:<40}  {:>6}  {:?}", "label", "c", "kind");
    for s in &sensors {
        println!("{:<40}  {:>6.1}  {:?}", s.label, s.celsius, s.kind);
    }
    println!("\n{} sensors", sensors.len());
}
