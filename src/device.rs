//! HID plumbing: locating the vendor configuration interface and pushing payloads to it.

use anyhow::{anyhow, Context, Result};
use hidapi::{DeviceInfo, HidApi, HidDevice};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use crate::protocol::{parse_battery, trace_fragments, Battery, CONFIG_REPORT_ID};

pub const VENDOR_ID: u16 = 0x093a;

/// Vendor usage page carrying the configuration feature report.
const VENDOR_USAGE_PAGE: u16 = 0xff00;
/// The configuration collection always lives on USB interface 1.
const CONFIG_INTERFACE: i32 = 1;

/// Known Pixart-based Glorious mice. `wireless` marks receiver/dongle product ids.
pub const SUPPORTED: &[(u16, &str, bool)] = &[
    (0x821a, "Model I 2 Wireless", true),
    (0x821d, "Model I 2 Wireless", false),
    (0x822a, "Model O 2 Wireless (wired)", false),
    (0x822b, "Model O 2 Bluetooth", true),
    (0x822d, "Model O 2 Wireless (receiver)", true),
    (0x826a, "Model O 2 Mini Wireless", false),
    (0x826d, "Model O 2 Mini Wireless (receiver)", true),
];

pub fn product_info(pid: u16) -> Option<(&'static str, bool)> {
    SUPPORTED
        .iter()
        .find(|(p, _, _)| *p == pid)
        .map(|(_, name, wireless)| (*name, *wireless))
}

fn is_supported(info: &DeviceInfo) -> bool {
    info.vendor_id() == VENDOR_ID && product_info(info.product_id()).is_some()
}

/// Ranks interfaces so the vendor configuration collection is tried first.
///
/// On Linux hidapi enumerates one entry per HID collection, so the same hidraw node appears
/// several times with different usage pages. Interface 1 + usage page 0xff00 is the one that
/// actually declares feature report 0x03.
fn rank(info: &DeviceInfo) -> (u8, i32) {
    let usage_rank = match (info.interface_number(), info.usage_page()) {
        (CONFIG_INTERFACE, VENDOR_USAGE_PAGE) => 0,
        (CONFIG_INTERFACE, page) if page >= 0xff00 => 1,
        (CONFIG_INTERFACE, _) => 2,
        (_, page) if page >= 0xff00 => 3,
        _ => 4,
    };
    (usage_rank, info.interface_number())
}

fn candidates(api: &HidApi) -> Vec<&DeviceInfo> {
    let mut found: Vec<&DeviceInfo> = api.device_list().filter(|d| is_supported(d)).collect();
    found.sort_by_key(|d| rank(d));
    found
}

pub struct Found {
    pub name: String,
    pub product_id: u16,
    pub wireless: bool,
    pub path: String,
    pub interface: i32,
    pub usage_page: u16,
}

pub fn find() -> Result<Found> {
    let api = HidApi::new().context("failed to initialize hidapi")?;
    let list = candidates(&api);
    let info = list.first().ok_or_else(no_device_error)?;
    let (name, wireless) = product_info(info.product_id()).unwrap_or(("Unknown", false));
    Ok(Found {
        name: info.product_string().unwrap_or(name).to_string(),
        product_id: info.product_id(),
        wireless,
        path: info.path().to_string_lossy().to_string(),
        interface: info.interface_number(),
        usage_page: info.usage_page(),
    })
}

fn no_device_error() -> anyhow::Error {
    anyhow!(
        "no supported Pixart Glorious mouse found (looked for VID {VENDOR_ID:04x}).\n\
         If the mouse is plugged in, run `glor doctor` to check hidraw permissions."
    )
}

/// Why the device could not be used, when it could not.
#[derive(Debug, PartialEq, Eq)]
pub enum AccessProblem {
    /// Nothing matching the supported vendor/product ids is connected.
    NoDevice,
    /// The device is present but its hidraw nodes are not writable by this user.
    PermissionDenied { paths: Vec<String> },
}

/// Classifies device access without going through hidapi.
///
/// hidapi reports failures as opaque strings, which is no basis for deciding whether to
/// offer privilege escalation. Opening the hidraw node directly gives a real `ErrorKind`.
pub fn access_problem() -> Option<AccessProblem> {
    let Ok(api) = HidApi::new() else {
        return Some(AccessProblem::NoDevice);
    };
    let paths: Vec<String> = candidates(&api)
        .iter()
        .map(|d| d.path().to_string_lossy().to_string())
        .collect();
    if paths.is_empty() {
        return Some(AccessProblem::NoDevice);
    }

    let mut denied = Vec::new();
    for path in paths {
        match fs::OpenOptions::new().read(true).write(true).open(&path) {
            Ok(_) => return None, // at least one node is usable
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                if !denied.contains(&path) {
                    denied.push(path);
                }
            }
            // Busy or transient: not a permission problem, so escalating would not help.
            Err(_) => return None,
        }
    }
    Some(AccessProblem::PermissionDenied { paths: denied })
}

/// The udev rule that makes the device usable without elevation.
pub const UDEV_RULES: &str = include_str!("../udev/70-glorious.rules");
pub const UDEV_RULE_NAME: &str = "70-glorious.rules";

/// Guidance printed when the device is present but unwritable.
pub fn permission_help() -> String {
    format!(
        "The mouse is connected but its hidraw node is not writable by your user.\n\
         \n\
         The durable fix is a udev rule. On most distributions:\n\
         \n\
         \x20 sudo cp {UDEV_RULE_NAME} /etc/udev/rules.d/\n\
         \x20 sudo udevadm control --reload-rules && sudo udevadm trigger\n\
         \n\
         On NixOS, add this repository's flake and enable the module:\n\
         \n\
         \x20 programs.glor.enable = true;\n\
         \n\
         Then replug the mouse (or its receiver) and no elevation is needed again."
    )
}

pub fn detect() -> Result<String> {
    let found = find()?;
    Ok(format!(
        "{} ({:04x}:{:04x}) — {} · {}\n  path={} interface={} usage_page=0x{:04x}",
        found.name,
        VENDOR_ID,
        found.product_id,
        if found.wireless { "wireless" } else { "wired" },
        "write-only config interface",
        found.path,
        found.interface,
        found.usage_page,
    ))
}

pub fn doctor() -> Result<String> {
    let api = HidApi::new().context("failed to initialize hidapi")?;
    let list = candidates(&api);
    if list.is_empty() {
        return Err(no_device_error());
    }

    let mut lines = vec![format!("{} candidate interface(s):", list.len())];
    let mut any_writable = false;
    for info in list {
        let path = info.path().to_string_lossy().to_string();
        let mode = fs::metadata(&path)
            .map(|m| format!("{:o}", m.permissions().mode() & 0o777))
            .unwrap_or_else(|_| "?".to_string());
        let open = match info.open_device(&api) {
            Ok(_) => {
                any_writable = true;
                "open=ok".to_string()
            }
            Err(e) => format!("open=FAIL ({e})"),
        };
        let preferred = if rank(info).0 == 0 {
            " <= preferred"
        } else {
            ""
        };
        lines.push(format!(
            "  {path} mode={mode} interface={} usage_page=0x{:04x} {open}{preferred}",
            info.interface_number(),
            info.usage_page(),
        ));
    }

    if !any_writable {
        lines.push(String::new());
        lines.push(permission_help());
        lines.push(String::new());
        lines.push("Print the rule with: glor doctor --udev-rule".to_string());
    }
    Ok(lines.join("\n"))
}

/// Pause between consecutive feature reports.
///
/// CORE waits 150 ms per report over a receiver (30 ms on Bluetooth). The firmware needs
/// the gap: each report is relayed over the 2.4 GHz link and written to flash, and a report
/// that arrives while the previous one is still being processed is dropped. Sending
/// back-to-back looks like it works for a single payload but silently loses writes when
/// several are sent in a row.
pub const INTER_FRAGMENT_DELAY: Duration = Duration::from_millis(150);

/// Extra settle time after a profile switch, which makes the mouse reload a whole
/// configuration from flash. CORE allows 550 ms before sending anything else.
pub const PROFILE_SETTLE_DELAY: Duration = Duration::from_millis(550);

fn write_fragments(device: &HidDevice, fragments: &[&[u8]]) -> Result<()> {
    for (idx, frag) in fragments.iter().enumerate() {
        debug_assert_eq!(frag[0], CONFIG_REPORT_ID);
        device
            .send_feature_report(frag)
            .with_context(|| format!("send_feature_report failed on fragment {idx}"))?;
        std::thread::sleep(INTER_FRAGMENT_DELAY);
    }
    Ok(())
}

/// Pushes a payload to the first interface that accepts it.
///
/// `trace` hexdumps the fragments to stderr before sending, which is how the payload layout
/// gets verified against captures.
pub fn apply(label: &str, fragments: &[&[u8]], trace: bool) -> Result<()> {
    if trace {
        eprint!("{}", trace_fragments(label, fragments));
    }

    let api = HidApi::new().context("failed to initialize hidapi")?;
    let list = candidates(&api);
    if list.is_empty() {
        return Err(no_device_error());
    }

    let mut errors = Vec::new();
    for info in list {
        let path = info.path().to_string_lossy().to_string();
        let attempt = info
            .open_device(&api)
            .context("open_device failed")
            .and_then(|dev| write_fragments(&dev, fragments));
        match attempt {
            Ok(()) => return Ok(()),
            Err(e) => errors.push(format!("{path}: {e:#}")),
        }
    }

    Err(anyhow!(
        "could not write to any interface:\n  {}\n\nRun `glor doctor` to check permissions.",
        errors.join("\n  ")
    ))
}

/// Switches the active onboard profile and waits for the mouse to finish reloading.
///
/// The settle is not optional: without it, the next payload arrives while the mouse is
/// still loading from flash and is silently dropped, leaving the device on its stored
/// configuration while the local cache believes otherwise.
pub fn switch_profile(profile: u8, trace: bool) -> Result<()> {
    let packet = crate::protocol::profile_switch_packet(profile);
    apply("profile", &[packet.as_slice()], trace)?;
    std::thread::sleep(PROFILE_SETTLE_DELAY);
    Ok(())
}

/// Interval at which the firmware broadcasts an unsolicited battery report.
///
/// Measured at 65.3 s between consecutive pushes across three intervals. A wait shorter
/// than this can miss the heartbeat entirely — which is exactly how earlier versions of this
/// tool concluded the device had no battery support at all.
pub const BATTERY_HEARTBEAT: Duration = Duration::from_secs(66);

/// Listens on the vendor interrupt endpoint for a battery report.
///
/// The firmware broadcasts these on its own roughly every [`BATTERY_HEARTBEAT`], and
/// additionally on a power-state change; they cannot be polled, so this blocks until one
/// arrives or `timeout` elapses. Movement reports share the endpoint and are skipped.
/// `on_tick` is called about once a second so callers can show progress during the wait.
pub fn listen_battery_with(
    timeout: Duration,
    mut on_tick: impl FnMut(Duration),
) -> Result<Option<Battery>> {
    let api = HidApi::new().context("failed to initialize hidapi")?;
    let list = candidates(&api);
    let info = list.first().ok_or_else(no_device_error)?;
    let dev = info
        .open_device(&api)
        .with_context(|| format!("opening {}", info.path().to_string_lossy()))?;

    let started = Instant::now();
    let deadline = started + timeout;
    let mut last_tick = started;
    let mut buf = [0u8; 64];
    while Instant::now() < deadline {
        // Short per-read timeout so the overall deadline and progress stay responsive.
        match dev.read_timeout(&mut buf, 250) {
            Ok(0) => {}
            Ok(n) => {
                if let Some(battery) = parse_battery(&buf[..n]) {
                    return Ok(Some(battery));
                }
            }
            Err(e) => return Err(anyhow!("reading from device: {e}")),
        }
        if last_tick.elapsed() >= Duration::from_secs(1) {
            last_tick = Instant::now();
            on_tick(started.elapsed());
        }
    }
    Ok(None)
}

/// A long-lived reader for the battery broadcast, for UIs that want live updates.
///
/// Holds one open handle and polls without blocking, so it never delays a configuration
/// write on the same device — unlike a blocking read loop, which starves them.
pub struct BatteryListener {
    device: HidDevice,
}

impl BatteryListener {
    pub fn open() -> Result<BatteryListener> {
        let api = HidApi::new().context("failed to initialize hidapi")?;
        let list = candidates(&api);
        let info = list.first().ok_or_else(no_device_error)?;
        let device = info
            .open_device(&api)
            .with_context(|| format!("opening {}", info.path().to_string_lossy()))?;
        device
            .set_blocking_mode(false)
            .context("switching to non-blocking reads")?;
        Ok(BatteryListener { device })
    }

    /// Drains whatever is queued and returns the most recent battery reading, if any.
    ///
    /// The endpoint also carries movement reports, which arrive constantly while the mouse
    /// is in use; draining a bounded number per call keeps up without stalling the caller.
    pub fn poll(&self) -> Option<Battery> {
        let mut latest = None;
        let mut buf = [0u8; 64];
        for _ in 0..64 {
            match self.device.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Some(battery) = parse_battery(&buf[..n]) {
                        latest = Some(battery);
                    }
                }
            }
        }
        latest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_help_renders() {
        let help = permission_help();
        println!(
            "
{help}
"
        );
        assert!(help.contains(UDEV_RULE_NAME));
        assert!(help.contains("programs.glor.enable"));
        assert!(help.contains("udevadm"));
    }

    #[test]
    fn udev_rules_cover_every_supported_device() {
        // The rule file and the device table are edited independently; a device added to
        // one but not the other would be silently unusable without root.
        for (pid, name, _) in SUPPORTED {
            let needle = format!("ATTRS{{idProduct}}==\"{pid:04x}\"");
            assert!(
                UDEV_RULES.contains(&needle),
                "{name} ({pid:04x}) is supported but missing from {UDEV_RULE_NAME}"
            );
        }
        assert!(UDEV_RULES.contains(&format!("ATTRS{{idVendor}}==\"{VENDOR_ID:04x}\"")));
    }

    #[test]
    fn known_products_resolve() {
        assert_eq!(
            product_info(0x822d).unwrap().0,
            "Model O 2 Wireless (receiver)"
        );
        assert!(product_info(0x822d).unwrap().1, "receiver is wireless");
        assert!(product_info(0x0000).is_none());
    }
}
