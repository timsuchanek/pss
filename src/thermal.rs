//! Native thermal sensor reader via IOHIDEventSystemClient.
//!
//! On Apple Silicon, Apple exposes labeled temperature sensors through the
//! IOHID event system (same path Stats.app, asitop, macmon use). We match on
//! PrimaryUsagePage=0xff00 / PrimaryUsage=0x0005 (AppleVendor temperature),
//! enumerate services, read each service's `Product` label and synchronous
//! temperature event. Works without sudo, stable across M1..M5 because Apple
//! ships labels (no magic 4-char SMC keys to maintain per SoC).

#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(not(target_os = "macos"))]
pub use stub::*;

#[cfg(not(target_os = "macos"))]
mod stub {
    #[derive(Clone, Debug)]
    pub struct Sensor {
        pub label: String,
        pub celsius: f32,
        pub kind: SensorKind,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum SensorKind {
        Cpu,
        Gpu,
        Ane,
        Memory,
        Battery,
        Other,
    }

    pub struct ThermalReader;

    impl ThermalReader {
        pub fn new() -> Option<Self> {
            None
        }
        pub fn read(&self) -> Vec<Sensor> {
            Vec::new()
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef};
    use core_foundation_sys::base::{CFAllocatorRef, CFRelease, CFTypeRef, kCFAllocatorDefault};
    use core_foundation_sys::dictionary::CFDictionaryRef;
    use core_foundation_sys::string::CFStringRef;

    // IOHID event-system bindings. Symbols are exported from IOKit.framework
    // but not in the public SDK headers — same surface every thermal tool on
    // macOS calls (Stats, macmon, asitop).
    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOHIDEventSystemClientCreate(allocator: CFAllocatorRef) -> CFTypeRef;
        fn IOHIDEventSystemClientSetMatching(client: CFTypeRef, matching: CFDictionaryRef);
        fn IOHIDEventSystemClientCopyServices(client: CFTypeRef) -> CFArrayRef;
        fn IOHIDServiceClientCopyProperty(
            service: CFTypeRef,
            property: CFStringRef,
        ) -> CFTypeRef;
        fn IOHIDServiceClientCopyEvent(
            service: CFTypeRef,
            ev_type: i64,
            options: i32,
            timestamp: i64,
        ) -> CFTypeRef;
        fn IOHIDEventGetFloatValue(event: CFTypeRef, field: i32) -> f64;
    }

    // kHIDPage_AppleVendor / kHIDUsage_AppleVendor_TemperatureSensor
    const HID_PAGE_APPLE_VENDOR: i32 = 0xff00;
    const HID_USAGE_APPLE_VENDOR_TEMP: i32 = 0x0005;
    // kIOHIDEventTypeTemperature = 15; field = base<<16 + 0.
    const EVENT_TYPE_TEMPERATURE: i64 = 15;
    const EVENT_FIELD_TEMPERATURE_LEVEL: i32 = (EVENT_TYPE_TEMPERATURE as i32) << 16;

    #[derive(Clone, Debug)]
    pub struct Sensor {
        pub label: String,
        pub celsius: f32,
        pub kind: SensorKind,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum SensorKind {
        Cpu,
        Gpu,
        Ane,
        Memory,
        Battery,
        Other,
    }

    fn classify(label: &str) -> SensorKind {
        let l = label.to_ascii_lowercase();
        // Order matters: check specific keywords before generic ones.
        // Labels vary by SoC generation:
        //   M1/M2 era:   "pACC MTR Temp Sensor0", "gpu die temp", "ANE MTR Temp"
        //   M3/M4/M5:    "PMU tdieN", "PMU tcal", "NAND CH0 temp", "gas gauge battery"
        if l.contains("gpu") {
            SensorKind::Gpu
        } else if l.contains("ane") {
            SensorKind::Ane
        } else if l.contains("batt") || l.contains("gauge") {
            SensorKind::Battery
        } else if l.contains("nand") {
            // Storage controller temp — closest semantic bucket is memory.
            SensorKind::Memory
        } else if l.contains("dram") || l.contains("memory") {
            SensorKind::Memory
        } else if l.contains("pmu")
            || l.contains("tdie")
            || l.contains("tcal")
            || l.contains("pacc")
            || l.contains("eacc")
            || l.contains("pcore")
            || l.contains("ecore")
            || l.contains("cpu")
            || l.contains("soc")
            || l.contains("isp")
        {
            SensorKind::Cpu
        } else {
            SensorKind::Other
        }
    }

    pub struct ThermalReader {
        client: CFTypeRef,
    }

    // SAFETY: we keep the client private and only call into it from one
    // polling thread. The IOHID client itself is documented as thread-safe
    // when used without a runloop dispatch, which matches our usage.
    unsafe impl Send for ThermalReader {}

    impl Drop for ThermalReader {
        fn drop(&mut self) {
            if !self.client.is_null() {
                unsafe { CFRelease(self.client) };
            }
        }
    }

    impl ThermalReader {
        pub fn new() -> Option<Self> {
            unsafe {
                let client = IOHIDEventSystemClientCreate(kCFAllocatorDefault);
                if client.is_null() {
                    return None;
                }
                let page_key = CFString::from_static_string("PrimaryUsagePage");
                let usage_key = CFString::from_static_string("PrimaryUsage");
                let page_val = CFNumber::from(HID_PAGE_APPLE_VENDOR);
                let usage_val = CFNumber::from(HID_USAGE_APPLE_VENDOR_TEMP);
                let pairs: Vec<(CFType, CFType)> = vec![
                    (page_key.as_CFType(), page_val.as_CFType()),
                    (usage_key.as_CFType(), usage_val.as_CFType()),
                ];
                let dict = CFDictionary::from_CFType_pairs(&pairs);
                IOHIDEventSystemClientSetMatching(client, dict.as_concrete_TypeRef() as _);
                Some(Self { client })
            }
        }

        pub fn read(&self) -> Vec<Sensor> {
            let mut out = Vec::new();
            unsafe {
                let services = IOHIDEventSystemClientCopyServices(self.client);
                if services.is_null() {
                    return out;
                }
                let name_key = CFString::from_static_string("Product");
                let count = CFArrayGetCount(services);
                for i in 0..count {
                    let svc = CFArrayGetValueAtIndex(services, i) as CFTypeRef;
                    if svc.is_null() {
                        continue;
                    }
                    let label = read_string_property(svc, name_key.as_concrete_TypeRef() as _)
                        .unwrap_or_else(|| "(unnamed)".to_string());
                    let event = IOHIDServiceClientCopyEvent(
                        svc,
                        EVENT_TYPE_TEMPERATURE,
                        0,
                        0,
                    );
                    if event.is_null() {
                        continue;
                    }
                    let c = IOHIDEventGetFloatValue(event, EVENT_FIELD_TEMPERATURE_LEVEL);
                    CFRelease(event);
                    if c.is_finite() && (-20.0..200.0).contains(&c) {
                        let kind = classify(&label);
                        out.push(Sensor {
                            label,
                            celsius: c as f32,
                            kind,
                        });
                    }
                }
                CFRelease(services as CFTypeRef);
            }
            out
        }
    }

    unsafe fn read_string_property(service: CFTypeRef, key: CFStringRef) -> Option<String> {
        unsafe {
            let val = IOHIDServiceClientCopyProperty(service, key);
            if val.is_null() {
                return None;
            }
            let s = CFString::wrap_under_create_rule(val as _);
            Some(s.to_string())
        }
    }
}

// Aggregate view the UI consumes.
#[derive(Clone, Debug, Default)]
pub struct ThermalSnapshot {
    pub sensors: Vec<Sensor>,
}

impl ThermalSnapshot {
    pub fn max_of(&self, kind: SensorKind) -> Option<f32> {
        self.sensors
            .iter()
            .filter(|s| s.kind == kind)
            .map(|s| s.celsius)
            .fold(None, |acc, v| Some(acc.map_or(v, |a: f32| a.max(v))))
    }
}
