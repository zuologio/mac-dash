//! SoC temperature without root, via IOHIDEventSystemClient.
//!
//! **This module uses a private API and is quarantined accordingly.**
//!
//! Apple Silicon exposes no documented unprivileged temperature interface:
//! `powermetrics` requires root, and the SMC keys that worked on Intel Macs
//! are gone. The remaining path is `IOHIDEventSystemClient`, matching on
//! usage page 0xff00 / usage 0x0005 and reading `kIOHIDEventTypeTemperature`
//! — the approach used by macmon and Stats.app.
//!
//! It is undocumented and may change in any macOS release. Everything here
//! therefore returns `Result`/`Option`: a failure yields `"temp": null` in the
//! packet while every other metric continues to flow. A private API is
//! allowed to break; it is not allowed to take the agent down with it.
//!
//! When it does break, `macdash-agent --list-sensors` dumps the raw sensor
//! names so the cause is visible in seconds rather than requiring a debugger.

use std::ffi::c_void;

use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFRelease, TCFType};
use core_foundation::dictionary::{CFDictionary, CFMutableDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use macdash_proto::Temp;

type IOHIDEventSystemClientRef = *mut c_void;
type IOHIDServiceClientRef = *mut c_void;
type IOHIDEventRef = *mut c_void;

const K_IOHID_EVENT_TYPE_TEMPERATURE: i64 = 15;
const PRIMARY_USAGE_PAGE_SENSOR: i32 = 0xff00;
const PRIMARY_USAGE_TEMPERATURE: i32 = 0x0005;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDEventSystemClientCreate(allocator: *const c_void) -> IOHIDEventSystemClientRef;
    fn IOHIDEventSystemClientSetMatching(
        client: IOHIDEventSystemClientRef,
        matching: CFMutableDictionaryRef,
    ) -> i32;
    fn IOHIDEventSystemClientCopyServices(client: IOHIDEventSystemClientRef) -> CFArrayRef;
    fn IOHIDServiceClientCopyProperty(
        service: IOHIDServiceClientRef,
        key: CFStringRef,
    ) -> *const c_void;
    fn IOHIDServiceClientCopyEvent(
        service: IOHIDServiceClientRef,
        event_type: i64,
        options: i32,
        timeout: u64,
    ) -> IOHIDEventRef;
    fn IOHIDEventGetFloatValue(event: IOHIDEventRef, field: i64) -> f64;
}

/// Field selector for a temperature event's value.
const fn temperature_field() -> i64 {
    K_IOHID_EVENT_TYPE_TEMPERATURE << 16
}

/// Maximum die/SoC sensors polled per sample.
///
/// The binding constraint is time, not information: each read is a blocking
/// ~500 µs IOKit round trip, so every additional sensor costs more than every
/// other metric in the packet combined. Adjacent die sensors on one package
/// track within a degree or two, so three is enough to find the hot spot for
/// a display that shows a single number.
const MAX_SOC_SENSORS: usize = 3;

/// Choose a small, stable subset of sensors to poll on the hot path.
///
/// Stable matters: re-selecting each sample would make the reported
/// temperature jump as different sensors came in and out of the set.
fn select_hot_sensors(
    all: &[(String, IOHIDServiceClientRef)],
) -> Vec<(String, IOHIDServiceClientRef)> {
    let mut hot = Vec::new();

    let mut take = |pred: &dyn Fn(&str) -> bool, limit: usize| {
        let mut n = 0;
        for (name, svc) in all {
            if n >= limit {
                break;
            }
            if pred(&name.to_lowercase())
                && !hot
                    .iter()
                    .any(|(existing, _): &(String, _)| existing == name)
            {
                hot.push((name.clone(), *svc));
                n += 1;
            }
        }
    };

    // Die sensors are what throttling responds to, so they get the budget.
    take(
        &|n| n.contains("tdie") || n.contains("soc") || n.contains("cpu"),
        MAX_SOC_SENSORS,
    );
    take(&|n| n.contains("gpu"), 1);
    take(&|n| n.contains("battery") || n.contains("gas gauge"), 1);

    if hot.is_empty() {
        // Unrecognised naming scheme: take a bounded sample of whatever
        // exists rather than reporting no temperature at all.
        hot.extend(all.iter().take(MAX_SOC_SENSORS).cloned());
    }

    hot
}

/// Minimum interval between hardware reads.
///
/// Temperature is a thermal mass. A CPU die under a sudden full load takes
/// seconds to climb, and the heatsink it is bonded to takes tens of seconds
/// — so a reading refreshed every 2 s is indistinguishable on screen from one
/// refreshed every 200 ms, while costing a tenth as much.
///
/// This is the single most important constant for the agent's CPU budget:
/// each hardware read is ~4.5 ms of blocking IOKit IPC, against ~60 µs for
/// every other metric combined. Every packet still carries a temperature; it
/// is simply at most 2 s old.
const TEMP_MIN_INTERVAL_MS: u64 = 2_000;

pub struct TempCollector {
    client: Option<SensorClient>,
    /// Set once the private API has failed, to stop retrying every tick. A
    /// failure here is structural (API changed, sensors absent in a VM), not
    /// transient, so retrying 5 times a second would only waste cycles.
    disabled: bool,
    cached: Option<Temp>,
    pub last_read_ms: u64,
}

struct SensorClient {
    client: IOHIDEventSystemClientRef,
    /// Sensors polled on the hot path.
    services: Vec<(String, IOHIDServiceClientRef)>,
    /// Every matched sensor, for `--list-sensors` diagnostics.
    all_services: Vec<(String, IOHIDServiceClientRef)>,
}

// The client is created and used from the collector thread only; the raw
// pointers never cross threads.
unsafe impl Send for SensorClient {}

impl Drop for SensorClient {
    fn drop(&mut self) {
        unsafe {
            if !self.client.is_null() {
                CFRelease(self.client as *const c_void);
            }
        }
    }
}

impl TempCollector {
    pub fn new() -> Self {
        match SensorClient::open() {
            Ok(c) => Self {
                client: Some(c),
                disabled: false,
                cached: None,
                last_read_ms: 0,
            },
            Err(e) => {
                eprintln!("macdash: thermal sensors unavailable ({e}); temp will be null");
                Self {
                    client: None,
                    disabled: true,
                    cached: None,
                    last_read_ms: 0,
                }
            }
        }
    }

    /// Cached temperature, refreshed at most once per `TEMP_MIN_INTERVAL_MS`.
    pub fn sample(&mut self, now_ms: u64) -> Option<Temp> {
        if self.disabled {
            return None;
        }
        if self.cached.is_some() && now_ms.saturating_sub(self.last_read_ms) < TEMP_MIN_INTERVAL_MS
        {
            return self.cached.clone();
        }
        let client = self.client.as_ref()?;
        let temp = client.read();
        self.last_read_ms = now_ms;
        self.cached = (!temp.is_empty()).then_some(temp);
        self.cached.clone()
    }

    /// Raw sensor dump for `--list-sensors`. The diagnostic that makes a
    /// future macOS change debuggable in seconds.
    pub fn list_sensors(&self) -> Vec<(String, f32)> {
        self.client
            .as_ref()
            .map(|c| c.read_everything())
            .unwrap_or_default()
    }
}

impl SensorClient {
    fn open() -> Result<Self, String> {
        unsafe {
            let client = IOHIDEventSystemClientCreate(std::ptr::null());
            if client.is_null() {
                return Err("IOHIDEventSystemClientCreate returned null".into());
            }

            let matching = CFDictionary::from_CFType_pairs(&[
                (
                    CFString::new("PrimaryUsagePage").as_CFType(),
                    CFNumber::from(PRIMARY_USAGE_PAGE_SENSOR).as_CFType(),
                ),
                (
                    CFString::new("PrimaryUsage").as_CFType(),
                    CFNumber::from(PRIMARY_USAGE_TEMPERATURE).as_CFType(),
                ),
            ]);
            IOHIDEventSystemClientSetMatching(
                client,
                matching.as_concrete_TypeRef() as CFMutableDictionaryRef,
            );

            let services_ref = IOHIDEventSystemClientCopyServices(client);
            if services_ref.is_null() {
                CFRelease(client as *const c_void);
                return Err("no thermal services matched".into());
            }
            let services: CFArray<*const c_void> = CFArray::wrap_under_create_rule(services_ref);

            let key = CFString::new("Product");
            let mut all = Vec::new();
            for i in 0..services.len() {
                let Some(svc) = services.get(i) else { continue };
                let svc = *svc as IOHIDServiceClientRef;
                let name_ref = IOHIDServiceClientCopyProperty(svc, key.as_concrete_TypeRef());
                let name = if name_ref.is_null() {
                    format!("sensor{i}")
                } else {
                    let s = CFString::wrap_under_create_rule(name_ref as CFStringRef);
                    s.to_string()
                };
                all.push((name, svc));
            }

            // Each sensor read is a blocking IOKit round trip costing
            // ~800 µs, and a Mac exposes 45 of them. Reading all 45 cost
            // 44 ms per sample — by far the largest cost in the system,
            // 1000x everything else combined.
            //
            // The dashboard shows *one* SoC number, so reading 23 die
            // sensors to take their maximum is 22 round trips spent on
            // resolution nobody sees. A small fixed sample is kept instead:
            // adjacent die sensors on the same package track within a degree
            // or two, so the hottest of a handful is the hottest of all for
            // display purposes.
            let hot = select_hot_sensors(&all);

            if all.is_empty() {
                CFRelease(client as *const c_void);
                return Err("thermal service list was empty".into());
            }

            Ok(Self {
                client,
                services: hot,
                all_services: all,
            })
        }
    }

    fn read_all(&self) -> Vec<(String, f32)> {
        self.read_list(&self.services)
    }

    /// Every sensor, including those skipped on the hot path. Used only by
    /// `--list-sensors`.
    fn read_everything(&self) -> Vec<(String, f32)> {
        self.read_list(&self.all_services)
    }

    fn read_list(&self, list: &[(String, IOHIDServiceClientRef)]) -> Vec<(String, f32)> {
        let mut out = Vec::new();
        for (name, svc) in list {
            unsafe {
                let event = IOHIDServiceClientCopyEvent(*svc, K_IOHID_EVENT_TYPE_TEMPERATURE, 0, 0);
                if event.is_null() {
                    continue;
                }
                let v = IOHIDEventGetFloatValue(event, temperature_field()) as f32;
                CFRelease(event as *const c_void);
                // Filter implausible readings: some services match the usage
                // page but report 0 or garbage when the sensor is asleep.
                if v > 0.0 && v < 150.0 {
                    out.push((name.clone(), v));
                }
            }
        }
        out
    }

    /// Bucket raw sensors into the three fields the protocol carries.
    ///
    /// Sensor naming varies by Apple Silicon generation. Observed on an M5:
    ///
    /// ```text
    /// PMU tdie1..13    SoC die temperature   <- what throttling responds to
    /// PMU2 tdie1..10   second power domain
    /// PMU tdev1..8     device/package sensors, cooler than the die
    /// PMU tcal         calibration reference, not a real temperature
    /// NAND CH0 temp    storage
    /// gas gauge battery
    /// ```
    ///
    /// `soc` takes the **maximum** die reading rather than the mean: with 20+
    /// die sensors, averaging smooths away exactly the hotspot worth seeing,
    /// and the hottest die point is the figure that drives throttling.
    fn read(&self) -> Temp {
        let all = self.read_all();
        if all.is_empty() {
            return Temp {
                soc: None,
                gpu: None,
                battery: None,
            };
        }

        let max_of = |pred: &dyn Fn(&str) -> bool| -> Option<f32> {
            all.iter()
                .filter(|(n, _)| pred(&n.to_lowercase()))
                .map(|(_, v)| *v)
                .fold(None, |acc: Option<f32>, v| {
                    Some(acc.map_or(v, |a: f32| a.max(v)))
                })
        };

        let gpu = max_of(&|n| n.contains("gpu"));
        let battery = max_of(&|n| n.contains("battery") || n.contains("gas gauge"));

        let soc = max_of(&|n| n.contains("tdie") || n.contains("soc") || n.contains("cpu"))
            // Older/other naming schemes.
            .or_else(|| max_of(&|n| n.contains("tdev") || n.contains("pmgr")))
            // Unrecognised entirely: the hottest sensor that is not storage,
            // battery or a calibration reference. Better a rough number than
            // none, but never one polluted by a 30C battery reading.
            .or_else(|| {
                max_of(&|n| {
                    !n.contains("battery")
                        && !n.contains("gas gauge")
                        && !n.contains("nand")
                        && !n.contains("tcal")
                })
            });

        Temp { soc, gpu, battery }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The contract that matters: whatever the private API does, this must
    /// return rather than panic or hang.
    #[test]
    fn collector_construction_never_panics() {
        let mut c = TempCollector::new();
        let _ = c.sample(0);
    }

    /// The bug this catches: an unrecognised naming scheme falling back to
    /// "mean of every sensor", which averages a 38C die with a 30C battery
    /// and a 35C SSD, reporting a SoC temperature that is wrong and always
    /// too low.
    #[test]
    fn soc_is_not_diluted_by_cooler_sensors() {
        let c = TempCollector::new();
        let all = c.list_sensors();
        if all.len() < 5 {
            return; // no sensors on this machine (VM/CI)
        }
        let mut c2 = TempCollector::new();
        let Some(t) = c2.sample(0) else { return };
        let Some(soc) = t.soc else { return };
        let hottest = all.iter().map(|(_, v)| *v).fold(f32::MIN, f32::max);
        let mean = all.iter().map(|(_, v)| *v).sum::<f32>() / all.len() as f32;
        assert!(
            soc >= mean,
            "soc {soc:.1}C is below the all-sensor mean {mean:.1}C, suggesting dilution"
        );
        assert!(
            soc <= hottest + 0.01,
            "soc {soc:.1}C exceeds hottest sensor {hottest:.1}C"
        );
    }

    #[test]
    fn readings_are_physically_plausible() {
        let mut c = TempCollector::new();
        if let Some(t) = c.sample(0) {
            for (label, v) in [("soc", t.soc), ("gpu", t.gpu), ("battery", t.battery)] {
                if let Some(v) = v {
                    assert!(
                        (5.0..120.0).contains(&v),
                        "{label} temperature {v}C is outside any plausible range"
                    );
                }
            }
        }
    }
}
