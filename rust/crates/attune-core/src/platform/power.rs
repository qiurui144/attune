//! Power-state probe (AC / battery / thermal) for the L_hw power-aware scheduling
//! policy (two-layer-scheduling spec §S1).
//!
//! WHY: on a laptop, background KB build (embedding/OCR/ASR) defaults to a GPU EP
//! (DirectML/OpenVINO-GPU, chosen for throughput) and would run the GPU flat-out —
//! draining the battery and thermal-throttling. The resource governor + EP selector
//! consume [`PowerState`] to throttle/pause background work and prefer the
//! energy-optimal NPU path on battery (vlm-llm-bench: NPU OCR same accuracy, lower
//! power). Probing NEVER panics — any failure degrades to [`PowerSource::Unknown`]
//! which callers treat as AC (conservative: a desktop/server is never wrongly slowed).
//!
//! Mirrors the OS-branch + graceful-fallback idiom of `platform/mod.rs`
//! (`HardwareProfile::detect`): `#[cfg]` per OS, file/command parse, `Option` returns.

/// Where the machine draws power. `Unknown` is treated as AC by policy callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum PowerSource {
    Ac,
    Battery,
    #[default]
    Unknown,
}

/// OS power plan / performance preference. Coarse — drives the energy-vs-perf tilt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum PowerProfile {
    Performance,
    #[default]
    Balanced,
    Saver,
}

/// A point-in-time snapshot of the machine's power/thermal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PowerState {
    pub source: PowerSource,
    /// Battery charge percent (0-100) when on/with a battery, else `None`.
    pub battery_pct: Option<u8>,
    pub profile: PowerProfile,
    /// True when a thermal zone is above the throttle threshold (downshift signal).
    pub thermal_pressure: bool,
}

impl Default for PowerState {
    fn default() -> Self {
        // Conservative default = behave as plugged-in (no throttle) when unprobed.
        PowerState {
            source: PowerSource::Unknown,
            battery_pct: None,
            profile: PowerProfile::Balanced,
            thermal_pressure: false,
        }
    }
}

impl PowerState {
    /// Policy helper: should background (non-interactive) work run at full tilt?
    /// `Unknown`/`Ac` → yes. `Battery` → energy-conscious (caller throttles).
    pub fn is_energy_constrained(&self) -> bool {
        matches!(self.source, PowerSource::Battery)
            || self.thermal_pressure
            || matches!(self.profile, PowerProfile::Saver)
    }

    /// Policy helper: hard stop for background work (very low battery or thermal crit).
    pub fn should_pause_background(&self, low_battery_pct: u8) -> bool {
        if self.thermal_pressure {
            return true;
        }
        matches!(self.source, PowerSource::Battery)
            && self.battery_pct.is_some_and(|p| p < low_battery_pct)
    }
}

/// Linux thermal-zone trip: above this milli-°C we report thermal pressure.
/// 90°C is a conservative "sustained throttle" line (most laptops trip 95-100°C).
#[cfg(target_os = "linux")]
const THERMAL_TRIP_MILLI_C: i64 = 90_000;

/// Probe the current power state. Never panics; returns [`PowerState::default`]
/// (`Unknown`) on any platform error so policy degrades to "behave as AC".
pub fn probe() -> PowerState {
    #[cfg(target_os = "linux")]
    {
        probe_linux()
    }
    #[cfg(target_os = "windows")]
    {
        probe_windows()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        PowerState::default()
    }
}

// ───────────────────────────── Linux ─────────────────────────────

#[cfg(target_os = "linux")]
fn probe_linux() -> PowerState {
    let mut st = PowerState::default();

    // AC adapter: /sys/class/power_supply/{AC,ADP,ACAD,*}/online == 1
    // Battery: first BAT*/capacity. Iterate the dir; type=Mains vs Battery.
    if let Ok(entries) = std::fs::read_dir("/sys/class/power_supply") {
        let mut saw_mains_online: Option<bool> = None;
        let mut battery_pct: Option<u8> = None;
        for e in entries.flatten() {
            let p = e.path();
            let kind = std::fs::read_to_string(p.join("type")).unwrap_or_default();
            let kind = kind.trim();
            if kind == "Mains" {
                if let Ok(s) = std::fs::read_to_string(p.join("online")) {
                    if let Some(on) = parse_online(&s) {
                        // any online mains ⇒ AC
                        saw_mains_online = Some(saw_mains_online.unwrap_or(false) || on);
                    }
                }
            } else if kind == "Battery" && battery_pct.is_none() {
                if let Ok(s) = std::fs::read_to_string(p.join("capacity")) {
                    battery_pct = parse_capacity(&s);
                }
            }
        }
        st.battery_pct = battery_pct;
        st.source = match saw_mains_online {
            Some(true) => PowerSource::Ac,
            Some(false) => PowerSource::Battery,
            // No mains entry but a battery exists ⇒ assume battery; else unknown.
            None if battery_pct.is_some() => PowerSource::Battery,
            None => PowerSource::Unknown,
        };
    }

    // Thermal: any thermal_zone above trip ⇒ pressure.
    if let Ok(zones) = std::fs::read_dir("/sys/class/thermal") {
        for z in zones.flatten() {
            let name = z.file_name();
            if !name.to_string_lossy().starts_with("thermal_zone") {
                continue;
            }
            if let Ok(s) = std::fs::read_to_string(z.path().join("temp")) {
                if let Some(milli) = parse_thermal_milli(&s) {
                    if milli >= THERMAL_TRIP_MILLI_C {
                        st.thermal_pressure = true;
                        break;
                    }
                }
            }
        }
    }

    // Linux exposes no portable "power plan"; we cannot distinguish Saver from
    // Balanced from sysfs alone, so report Balanced regardless of source until a
    // real power-plan probe lands. (Both branches were already Balanced — collapse
    // to satisfy clippy::if_same_then_else without changing behavior.)
    st.profile = PowerProfile::Balanced;
    st
}

// ───────────────────────────── Windows ─────────────────────────────

#[cfg(target_os = "windows")]
fn probe_windows() -> PowerState {
    let mut st = PowerState::default();
    // Win32_Battery.BatteryStatus: 1=Discharging(on battery), 2=AC.
    // EstimatedChargeRemaining: 0-100. No FFI — match the wmic idiom used by
    // HardwareProfile::detect (wmic_cpu_info / wmic_total_physical_memory).
    use std::process::Command;
    if let Ok(out) = Command::new("wmic")
        .args([
            "path",
            "Win32_Battery",
            "get",
            "BatteryStatus,EstimatedChargeRemaining",
            "/value",
        ])
        .output()
    {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            let (src, pct) = parse_win_battery(&text);
            st.source = src;
            st.battery_pct = pct;
        }
    }
    // Desktops report no Win32_Battery row ⇒ parse_win_battery → (Unknown, None).
    // Treat unknown-on-Windows as AC (a batteryless box is plugged in).
    if st.source == PowerSource::Unknown {
        st.source = PowerSource::Ac;
    }
    // Power plan (best-effort): powercfg active scheme GUID → profile.
    if let Ok(out) = Command::new("powercfg").args(["/getactivescheme"]).output() {
        if out.status.success() {
            st.profile = parse_win_power_plan(&String::from_utf8_lossy(&out.stdout));
        }
    }
    st
}

// ───────────────────────────── pure parse helpers (testable) ─────────────────────────────

/// `/sys/.../online` → "1\n" = true, "0" = false.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_online(s: &str) -> Option<bool> {
    match s.trim() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

/// `/sys/.../capacity` → "0".."100".
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_capacity(s: &str) -> Option<u8> {
    s.trim().parse::<u8>().ok().map(|v| v.min(100))
}

/// `/sys/class/thermal/thermal_zoneN/temp` → milli-°C integer.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_thermal_milli(s: &str) -> Option<i64> {
    s.trim().parse::<i64>().ok()
}

/// Parse `wmic path Win32_Battery ... /value` output → (source, pct).
/// BatteryStatus 2 == AC connected; 1 == discharging. Absent rows → (Unknown, None).
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_win_battery(text: &str) -> (PowerSource, Option<u8>) {
    let mut status: Option<u32> = None;
    let mut pct: Option<u8> = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("BatteryStatus=") {
            status = v.trim().parse::<u32>().ok();
        } else if let Some(v) = line.strip_prefix("EstimatedChargeRemaining=") {
            pct = v.trim().parse::<u8>().ok().map(|v| v.min(100));
        }
    }
    let source = match status {
        // 2 = AC; 3/4/5 (fully-charged/low/critical on AC) also imply mains.
        Some(2) | Some(3) | Some(6) | Some(7) | Some(8) | Some(9) => PowerSource::Ac,
        Some(1) => PowerSource::Battery,
        Some(_) => PowerSource::Unknown,
        None => PowerSource::Unknown,
    };
    (source, pct)
}

/// `powercfg /getactivescheme` line → profile by well-known GUID.
/// Power saver `a1841308-...`, High performance `8c5e7fda-...`, Balanced default.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
fn parse_win_power_plan(text: &str) -> PowerProfile {
    let t = text.to_ascii_lowercase();
    if t.contains("a1841308-3541-4fab-bc81-f71556f20b4a") {
        PowerProfile::Saver
    } else if t.contains("8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c") {
        PowerProfile::Performance
    } else {
        PowerProfile::Balanced
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_unknown_unconstrained() {
        let d = PowerState::default();
        assert_eq!(d.source, PowerSource::Unknown);
        assert!(!d.is_energy_constrained());
        assert!(!d.should_pause_background(20));
    }

    #[test]
    fn battery_is_energy_constrained() {
        let st = PowerState {
            source: PowerSource::Battery,
            battery_pct: Some(55),
            profile: PowerProfile::Balanced,
            thermal_pressure: false,
        };
        assert!(st.is_energy_constrained());
        assert!(!st.should_pause_background(20)); // 55% > 20% floor
    }

    #[test]
    fn low_battery_and_thermal_pause() {
        let low = PowerState {
            source: PowerSource::Battery,
            battery_pct: Some(15),
            profile: PowerProfile::Balanced,
            thermal_pressure: false,
        };
        assert!(low.should_pause_background(20)); // 15% < 20%

        let hot = PowerState {
            source: PowerSource::Ac,
            battery_pct: None,
            profile: PowerProfile::Performance,
            thermal_pressure: true,
        };
        assert!(hot.should_pause_background(20)); // thermal crit even on AC
        assert!(hot.is_energy_constrained());
    }

    #[test]
    fn ac_saver_is_constrained_but_not_paused() {
        let st = PowerState {
            source: PowerSource::Ac,
            battery_pct: None,
            profile: PowerProfile::Saver,
            thermal_pressure: false,
        };
        assert!(st.is_energy_constrained()); // saver profile
        assert!(!st.should_pause_background(20));
    }

    #[test]
    fn linux_sys_parsers() {
        assert_eq!(parse_online("1\n"), Some(true));
        assert_eq!(parse_online("0"), Some(false));
        assert_eq!(parse_online("x"), None);
        assert_eq!(parse_capacity("87\n"), Some(87));
        assert_eq!(parse_capacity("130"), Some(100)); // clamp
        assert_eq!(parse_capacity(""), None);
        assert_eq!(parse_thermal_milli("91000\n"), Some(91000));
        assert!(parse_thermal_milli("91000").unwrap() >= 90_000);
    }

    #[test]
    fn win_battery_parser() {
        let ac = "BatteryStatus=2\r\nEstimatedChargeRemaining=90\r\n";
        assert_eq!(parse_win_battery(ac), (PowerSource::Ac, Some(90)));
        let bat = "BatteryStatus=1\r\nEstimatedChargeRemaining=42\r\n";
        assert_eq!(parse_win_battery(bat), (PowerSource::Battery, Some(42)));
        // desktop: no battery rows
        assert_eq!(parse_win_battery("\r\n\r\n"), (PowerSource::Unknown, None));
    }

    #[test]
    fn win_power_plan_parser() {
        assert_eq!(
            parse_win_power_plan("Power Scheme GUID: a1841308-3541-4fab-bc81-f71556f20b4a (Power saver)"),
            PowerProfile::Saver
        );
        assert_eq!(
            parse_win_power_plan("GUID: 8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c (High performance)"),
            PowerProfile::Performance
        );
        assert_eq!(parse_win_power_plan("GUID: 381b4222-... (Balanced)"), PowerProfile::Balanced);
    }
}
