//! On-disk mirror of the mouse's configuration.
//!
//! The Pixart firmware is write-only and both payloads are all-or-nothing: sending the
//! settings payload to change debounce also rewrites DPI, polling rate, LOD and every
//! stage colour. Without a local mirror, changing one setting silently resets the rest.
//! Every mutation is therefore load-modify-write against this file.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::buttons::{Buttons, Macro};
use crate::protocol::{
    Battery, Lighting, Settings, DPI_MAX, DPI_MIN, DPI_UNIT, LOD_HIGH_MM, LOD_MEDIUM_MM,
    MAX_STAGES, MIN_STAGES, PROFILE_COUNT, SPEED_RAW_MAX,
};

/// Bumped to 2 when the single lighting/settings pair became a list of profiles.
const STATE_VERSION: u32 = 2;

/// The most recent battery reading, with when it was seen.
///
/// Battery reports arrive only on a power-state change and cannot be polled, so a reading
/// is cached and always displayed with its age — a stale value presented as current would
/// be worse than none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryReading {
    pub percent: u8,
    pub charging: bool,
    /// Seconds since the Unix epoch.
    pub seen_at: u64,
}

impl BatteryReading {
    pub fn new(battery: Battery) -> Self {
        BatteryReading {
            percent: battery.percent,
            charging: battery.charging,
            seen_at: now_unix(),
        }
    }

    /// Seconds since this reading was captured.
    pub fn age_secs(&self) -> u64 {
        now_unix().saturating_sub(self.seen_at)
    }

    /// Human-readable age, e.g. "3m ago".
    pub fn age(&self) -> String {
        let secs = self.age_secs();
        match secs {
            0..=59 => format!("{secs}s ago"),
            60..=3599 => format!("{}m ago", secs / 60),
            3600..=86399 => format!("{}h ago", secs / 3600),
            _ => format!("{}d ago", secs / 86400),
        }
    }
}

/// Hands a file written under `sudo` back to the user who invoked it.
///
/// Without this, one elevated run leaves a root-owned `state.json` and every later
/// unprivileged run fails to save — turning a one-off permission workaround into a
/// permanent breakage. Does nothing when not running under sudo.
fn restore_invoking_owner(path: &Path) {
    let (Some(uid), Some(gid)) = (env_id("SUDO_UID"), env_id("SUDO_GID")) else {
        return;
    };
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
        return;
    };
    // SAFETY: c_path is a valid NUL-terminated path; chown failure is reported via return
    // value, which is deliberately ignored — a best-effort fixup must not fail the save.
    unsafe {
        libc::chown(c_path.as_ptr(), uid, gid);
    }
    // The parent directory may also have been created as root on the first elevated run.
    if let Some(parent) = path.parent() {
        if let Ok(c_parent) = std::ffi::CString::new(parent.as_os_str().as_encoded_bytes()) {
            unsafe {
                libc::chown(c_parent.as_ptr(), uid, gid);
            }
        }
    }
}

fn env_id(key: &str) -> Option<u32> {
    std::env::var(key).ok()?.parse().ok()
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// One onboard profile. The device stores lighting and settings per profile, and every
/// payload carries the profile index it applies to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub lighting: Lighting,
    pub settings: Settings,
    pub buttons: Buttons,
    /// Macros this profile's bindings can reference, by id.
    pub macros: Vec<Macro>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    /// Zero-indexed active profile.
    #[serde(default)]
    pub active_profile: u8,
    #[serde(default = "default_profiles")]
    pub profiles: Vec<Profile>,
    /// Last observed battery reading, if one has ever been captured.
    #[serde(default)]
    pub battery: Option<BatteryReading>,

    // --- migration from the single-profile layout (state version 1) ---
    /// Present only in v1 files, where lighting sat at the top level. Folded into
    /// `profiles[0]` by `normalize` and never written back.
    #[serde(default, rename = "lighting", skip_serializing)]
    legacy_lighting: Option<Lighting>,
    /// Present only in v1 files. See `legacy_lighting`.
    #[serde(default, rename = "settings", skip_serializing)]
    legacy_settings: Option<Settings>,
}

fn default_profiles() -> Vec<Profile> {
    vec![Profile::default(); PROFILE_COUNT as usize]
}

impl Default for State {
    fn default() -> Self {
        State {
            version: STATE_VERSION,
            active_profile: 0,
            profiles: default_profiles(),
            battery: None,
            legacy_lighting: None,
            legacy_settings: None,
        }
    }
}

impl State {
    /// The profile every command acts on unless told otherwise.
    pub fn active(&self) -> &Profile {
        &self.profiles[self.active_profile as usize]
    }

    pub fn active_mut(&mut self) -> &mut Profile {
        let idx = self.active_profile as usize;
        &mut self.profiles[idx]
    }

    /// Borrows a specific profile, or the active one when `profile` is `None`.
    pub fn profile_mut(&mut self, profile: Option<u8>) -> Result<(&mut Profile, u8)> {
        let idx = profile.unwrap_or(self.active_profile);
        if idx >= PROFILE_COUNT {
            anyhow::bail!("profile {} does not exist (1-{PROFILE_COUNT})", idx + 1);
        }
        Ok((&mut self.profiles[idx as usize], idx))
    }
}

/// `$GLOR_STATE` overrides everything (used by the tests); otherwise
/// `$XDG_CONFIG_HOME/glor/state.json`, falling back to `~/.config/glor/state.json`.
pub fn state_path() -> Result<PathBuf> {
    if let Some(explicit) = std::env::var_os("GLOR_STATE") {
        return Ok(PathBuf::from(explicit));
    }
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => {
            let home =
                std::env::var_os("HOME").context("neither XDG_CONFIG_HOME nor HOME is set")?;
            PathBuf::from(home).join(".config")
        }
    };
    Ok(base.join("glor").join("state.json"))
}

impl State {
    /// Reads the cached state, falling back to factory defaults when it is missing.
    ///
    /// A corrupt file is reported rather than silently discarded, so a bad write never
    /// quietly resets the user's whole configuration.
    pub fn load() -> Result<State> {
        Self::load_from(&state_path()?)
    }

    pub fn load_from(path: &Path) -> Result<State> {
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(State::default()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let mut state: State = serde_json::from_str(&raw).with_context(|| {
            format!(
                "{} is not valid glor state; delete it to start from factory defaults",
                path.display()
            )
        })?;
        state.normalize();
        Ok(state)
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&state_path()?)
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self)?;
        // Write-then-rename so an interrupted save cannot truncate a good state file.
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        restore_invoking_owner(path);
        Ok(())
    }

    /// Repairs out-of-range values so a hand-edited or older state file can never build a
    /// payload the firmware would reject, and migrates the pre-profile layout.
    pub fn normalize(&mut self) {
        self.version = STATE_VERSION;
        // Resize first: a hand-edited file could carry an empty or short profile list, and
        // the migration below indexes profile 0.
        self.profiles
            .resize(PROFILE_COUNT as usize, Profile::default());

        // v1 files stored a single lighting/settings pair at the top level. Fold it into
        // profile 0 rather than discarding the user's configuration.
        if let Some(lighting) = self.legacy_lighting.take() {
            self.profiles[0].lighting = lighting;
        }
        if let Some(settings) = self.legacy_settings.take() {
            self.profiles[0].settings = settings;
        }

        if self.active_profile >= PROFILE_COUNT {
            self.active_profile = 0;
        }
        for profile in &mut self.profiles {
            profile.normalize();
        }
    }
}

impl Profile {
    fn normalize(&mut self) {
        let l = &mut self.lighting;
        l.brightness_wired = l.brightness_wired.min(SPEED_RAW_MAX);
        l.brightness_wireless = l.brightness_wireless.min(SPEED_RAW_MAX);
        l.speed = l.speed.min(SPEED_RAW_MAX);
        if let Some(count) = l.color_count {
            l.color_count = Some(count.clamp(1, 7));
        }

        let s = &mut self.settings;
        s.stage_count = (s.stage_count as usize).clamp(MIN_STAGES, MAX_STAGES) as u8;
        if s.active_stage >= s.stage_count {
            s.active_stage = 0;
        }
        s.lod_mm = if s.lod_mm == LOD_HIGH_MM {
            LOD_HIGH_MM
        } else {
            LOD_MEDIUM_MM
        };
        s.debounce_ms = crate::protocol::sanitize_debounce(s.debounce_ms);
        if crate::protocol::polling_hz_for(s.polling_code).is_none() {
            // Fall back to the fastest rate, not a bare 0x01 — under CORE's mapping that
            // code is 125 Hz, so a hardcoded literal here would quietly downgrade the mouse.
            s.polling_code = crate::protocol::polling_code_for(1000).expect("1000 Hz is valid");
        }
        for (idx, dpi) in s.stage_dpis.iter_mut().enumerate() {
            if idx < s.stage_count as usize {
                let rounded = (*dpi / DPI_UNIT) * DPI_UNIT;
                *dpi = rounded.clamp(DPI_MIN, DPI_MAX);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Effect;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("glor-test-{}-{}.json", name, std::process::id()))
    }

    #[test]
    fn missing_file_yields_defaults() {
        let path = temp_path("missing");
        let _ = fs::remove_file(&path);
        let state = State::load_from(&path).unwrap();
        assert_eq!(state, State::default());
    }

    #[test]
    fn round_trips_through_disk() {
        let path = temp_path("roundtrip");
        let mut state = State::default();
        state.active_mut().lighting.effect = Effect::Wave;
        state.active_mut().settings.stage_dpis[1] = 1600;
        state.save_to(&path).unwrap();

        let loaded = State::load_from(&path).unwrap();
        assert_eq!(loaded.active().lighting.effect, Effect::Wave);
        assert_eq!(loaded.active().settings.stage_dpis[1], 1600);
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn corrupt_state_errors_instead_of_resetting() {
        let path = temp_path("corrupt");
        fs::write(&path, "{ not json").unwrap();
        let err = State::load_from(&path).unwrap_err();
        assert!(err.to_string().contains("not valid glor state"));
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn normalize_repairs_out_of_range_values() {
        let mut state = State::default();
        state.active_mut().settings.stage_count = 9;
        state.active_mut().settings.active_stage = 8;
        state.active_mut().settings.debounce_ms = 99;
        state.active_mut().settings.polling_code = 0x7f;
        state.active_mut().settings.lod_mm = 7;
        state.active_mut().lighting.speed = 0xff;
        state.normalize();

        assert_eq!(state.active_mut().settings.stage_count, MAX_STAGES as u8);
        assert_eq!(state.active_mut().settings.active_stage, 0);
        assert_eq!(state.active_mut().settings.debounce_ms, 16);
        assert_eq!(
            state.active_mut().settings.polling_hz(),
            1000,
            "an invalid polling code falls back to the fastest rate, not to 125 Hz"
        );
        assert_eq!(state.active_mut().settings.lod_mm, LOD_MEDIUM_MM);
        assert_eq!(state.active_mut().lighting.speed, SPEED_RAW_MAX);
    }

    #[test]
    fn changing_one_setting_preserves_the_others() {
        // The regression this whole module exists to prevent.
        let path = temp_path("preserve");
        let mut state = State::default();
        state.active_mut().settings.stage_dpis = [1200, 2400, 3600, 4800, 0, 0];
        state.active_mut().settings.polling_code = crate::protocol::polling_code_for(500).unwrap();
        state.save_to(&path).unwrap();

        let mut reloaded = State::load_from(&path).unwrap();
        reloaded.active_mut().settings.debounce_ms = 4;
        reloaded.save_to(&path).unwrap();

        let final_state = State::load_from(&path).unwrap();
        let s = &final_state.active().settings;
        assert_eq!(s.debounce_ms, 4);
        assert_eq!(s.stage_dpis[0], 1200);
        assert_eq!(s.polling_hz(), 500);
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn v1_state_migrates_into_profile_zero() {
        // Files written before profiles existed kept lighting/settings at the top level.
        // They must be folded into profile 0, not silently discarded.
        let path = temp_path("v1-migrate");
        fs::write(
            &path,
            r#"{"version":1,
                "lighting":{"effect":"wave","speed":7},
                "settings":{"debounce_ms":6,"polling_code":3}}"#,
        )
        .unwrap();

        let state = State::load_from(&path).unwrap();
        assert_eq!(state.version, STATE_VERSION);
        assert_eq!(state.profiles.len(), PROFILE_COUNT as usize);
        assert_eq!(state.profiles[0].lighting.effect, Effect::Wave);
        assert_eq!(state.profiles[0].lighting.speed, 7);
        assert_eq!(state.profiles[0].settings.debounce_ms, 6);
        assert_eq!(state.profiles[0].settings.polling_hz(), 500);
        // Untouched profiles keep factory defaults.
        assert_eq!(state.profiles[1], Profile::default());
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn migrated_state_does_not_write_the_legacy_keys_back() {
        let path = temp_path("v1-roundtrip");
        fs::write(&path, r#"{"version":1,"lighting":{"effect":"rave"}}"#).unwrap();
        let state = State::load_from(&path).unwrap();
        state.save_to(&path).unwrap();

        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"profiles\""));
        assert!(
            !raw.contains("\n  \"lighting\""),
            "top-level lighting must not be re-serialised"
        );
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_short_profile_list_is_padded() {
        let path = temp_path("short-profiles");
        fs::write(&path, r#"{"version":2,"profiles":[],"active_profile":2}"#).unwrap();
        let state = State::load_from(&path).unwrap();
        assert_eq!(state.profiles.len(), PROFILE_COUNT as usize);
        assert_eq!(state.active_profile, 2, "still a valid index after padding");
        fs::remove_file(&path).unwrap();
    }
}
