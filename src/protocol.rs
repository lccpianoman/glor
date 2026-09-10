//! Pure encoding for the Glorious Model O 2 / I 2 family (Pixart firmware, VID 0x093a).
//!
//! The configuration interface is **write-only**: the firmware accepts payloads assembled
//! from 64-byte feature reports (report ID 0x03) on USB interface 1, and never echoes
//! settings back. Everything here is therefore a pure encoder; current state lives in
//! [`crate::state`].
//!
//! Layout cross-checked against three independent implementations:
//! - <https://github.com/zeppybabe/gloriousctl-linux> (`gloriousctl.c` payload structs)
//! - <https://github.com/OpenMouse-Project/mouse-protocol> (`src/glorious/index.ts`)
//! - `~/Code/glorious-ctl` (`mouse.py`, validated on this hardware)
//!
//! **All three disagree with this hardware on two lighting bytes.** Speed lives at [6], not
//! [9], and brightness at [10], not [6]/[7]. See the [`Lighting`] docs for the bisect that
//! established this.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Feature report ID carrying every configuration payload.
pub const CONFIG_REPORT_ID: u8 = 0x03;
/// Every fragment is exactly one 64-byte feature report.
pub const PACKET_LEN: usize = 64;

const LIGHTING_CMD: [u8; 2] = [0x02, 0xfb];
const LIGHTING_FRAGMENTS: usize = 3;
const SETTINGS_CMD: [u8; 2] = [0x04, 0xfb];
const SETTINGS_FRAGMENTS: usize = 4;

const PROFILE_CMD: [u8; 2] = [0x01, 0xfb];

/// Onboard profiles on this family. CORE reports both `minimumProfilesCount` and
/// `maximumIterableProfilesCount` as 3.
pub const PROFILE_COUNT: u8 = 3;

/// Byte [4] of every payload is `profileIndex + 1`, never a bare constant. Earlier versions
/// hardcoded `0x01`, which happened to work because that is profile 0.
fn profile_byte(profile: u8) -> u8 {
    profile.min(PROFILE_COUNT - 1) + 1
}

/// Builds the profile-switch packet (`cmd 01 fb`), which has no payload.
pub fn profile_switch_packet(profile: u8) -> [u8; PACKET_LEN] {
    let mut packet = [0u8; PACKET_LEN];
    packet[0] = CONFIG_REPORT_ID;
    packet[1] = PROFILE_CMD[0];
    packet[2] = PROFILE_CMD[1];
    packet[3] = profile_byte(profile);
    packet
}
/// Maximum brightness, and the value every reference implementation pins byte [10] to
/// while calling it a "modifier". On this hardware [10] is the brightness field.
pub const LIGHTING_MODIFIER: u8 = 0x14;

// ---------------------------------------------------------------------------
// Colors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const BLACK: Rgb = Rgb { r: 0, g: 0, b: 0 };

    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Rgb { r, g, b }
    }
}

impl fmt::Display for Rgb {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

impl FromStr for Rgb {
    type Err = anyhow::Error;

    /// Accepts `rrggbb`, `#rrggbb`, `rgb` and `#rgb`.
    fn from_str(s: &str) -> Result<Self> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        let expand = |c: u8| -> u8 {
            let v = (c as char).to_digit(16).unwrap_or(0) as u8;
            v << 4 | v
        };
        match hex.len() {
            6 => {
                let v = u32::from_str_radix(hex, 16)
                    .map_err(|_| anyhow::anyhow!("invalid hex color '{s}'"))?;
                Ok(Rgb::new((v >> 16) as u8, (v >> 8) as u8, v as u8))
            }
            3 if hex.bytes().all(|c| (c as char).is_ascii_hexdigit()) => {
                let b = hex.as_bytes();
                Ok(Rgb::new(expand(b[0]), expand(b[1]), expand(b[2])))
            }
            _ => bail!("invalid hex color '{s}' (expected rrggbb or rgb)"),
        }
    }
}

/// The factory rainbow the stock firmware ships with, used for multi-colour effects.
pub const DEFAULT_PALETTE: [Rgb; 7] = [
    Rgb::new(0xff, 0x00, 0x00),
    Rgb::new(0xff, 0x7f, 0x00),
    Rgb::new(0xff, 0xff, 0x00),
    Rgb::new(0x00, 0xff, 0x00),
    Rgb::new(0x00, 0x00, 0xff),
    Rgb::new(0x4b, 0x00, 0x82),
    Rgb::new(0x94, 0x00, 0xd3),
];

/// Factory per-stage DPI indicator colours (red, blue, green, yellow, then unset).
pub const DEFAULT_STAGE_COLORS: [Rgb; 6] = [
    Rgb::new(0xff, 0x00, 0x00),
    Rgb::new(0x00, 0x00, 0xff),
    Rgb::new(0x00, 0xff, 0x00),
    Rgb::new(0xff, 0xff, 0x00),
    Rgb::BLACK,
    Rgb::BLACK,
];

// ---------------------------------------------------------------------------
// Effects
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Effect {
    Off,
    /// Firmware's built-in rainbow wave.
    Glorious,
    SeamlessBreathing,
    Breathing,
    /// Static single colour.
    Solid,
    BreathingSingle,
    Tail,
    Rave,
    Wave,
}

impl Effect {
    pub const ALL: [Effect; 9] = [
        Effect::Off,
        Effect::Glorious,
        Effect::SeamlessBreathing,
        Effect::Breathing,
        Effect::Solid,
        Effect::BreathingSingle,
        Effect::Tail,
        Effect::Rave,
        Effect::Wave,
    ];

    pub fn id(self) -> u8 {
        match self {
            Effect::Off => 0x00,
            Effect::Glorious => 0x01,
            Effect::SeamlessBreathing => 0x02,
            Effect::Breathing => 0x03,
            Effect::Solid => 0x04,
            Effect::BreathingSingle => 0x05,
            Effect::Tail => 0x06,
            Effect::Rave => 0x07,
            Effect::Wave => 0x08,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Effect::Off => "off",
            Effect::Glorious => "glorious",
            Effect::SeamlessBreathing => "seamless-breathing",
            Effect::Breathing => "breathing",
            Effect::Solid => "solid",
            Effect::BreathingSingle => "breathing-single",
            Effect::Tail => "tail",
            Effect::Rave => "rave",
            Effect::Wave => "wave",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Effect::Off => "LEDs off",
            Effect::Glorious => "Glorious (rainbow wave)",
            Effect::SeamlessBreathing => "Seamless breathing",
            Effect::Breathing => "Breathing (multi-colour)",
            Effect::Solid => "Solid colour",
            Effect::BreathingSingle => "Breathing (single colour)",
            Effect::Tail => "Tail",
            Effect::Rave => "Rave",
            Effect::Wave => "Wave",
        }
    }

    /// How many palette slots the effect consumes (payload byte [8]).
    pub fn palette_slots(self) -> usize {
        match self {
            Effect::Rave => 2,
            Effect::Off | Effect::Solid | Effect::BreathingSingle => 1,
            _ => 7,
        }
    }

    /// Whether the effect animates, i.e. whether `speed` is meaningful.
    pub fn is_animated(self) -> bool {
        !matches!(self, Effect::Off | Effect::Solid)
    }

    /// Whether the user's chosen colours are used, as opposed to a firmware-fixed palette.
    pub fn uses_palette(self) -> bool {
        !matches!(self, Effect::Off | Effect::Glorious)
    }
}

impl FromStr for Effect {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let key = s.trim().to_ascii_lowercase().replace('_', "-");
        // Aliases keep older glor/glorious-ctl spellings working.
        let key = match key.as_str() {
            "cycle" => "seamless-breathing",
            "pulse" => "breathing",
            "pulse-one" | "single" | "breathing1" => "breathing-single",
            "normally-on" | "normallyon" => "solid",
            "breathing7" | "seamless" => "seamless-breathing",
            other => other,
        };
        Effect::ALL
            .into_iter()
            .find(|e| e.name() == key)
            .ok_or_else(|| anyhow::anyhow!("unknown effect '{s}'"))
    }
}

// ---------------------------------------------------------------------------
// Scalar encodings
// ---------------------------------------------------------------------------

/// Top of the firmware's speed and brightness scale. Both are continuous 0-20.
///
/// The four/five discrete "levels" that `gloriousctl-linux` and OpenMouse expose do not
/// exist in the protocol — CORE computes `value * 20 / 100` and sends the result directly.
pub const SPEED_RAW_MAX: u8 = 20;

pub const DPI_UNIT: u16 = 50;
pub const DPI_MIN: u16 = 100;
pub const DPI_MAX: u16 = 26_000;
pub const MAX_STAGES: usize = 6;
pub const MIN_STAGES: usize = 4;
pub const DEBOUNCE_MAX_MS: u8 = 16;

/// Lift-off distance, payload byte [7]. The 0.7 mm setting is not exposed by the firmware.
pub const LOD_MEDIUM_MM: u8 = 1;
pub const LOD_HIGH_MM: u8 = 2;

/// `(wire code, hertz)` for payload byte [9] of the settings payload.
///
/// Taken from CORE, which indexes `[125, 250, 500, 1000]` into `[1, 2, 3, 4]`.
/// `gloriousctl-linux` documents this as `0x01(1k), 0x02(125), 0x03(250), 0x04(500)` — a
/// rotation of the real mapping, which silently turns a request for 1000 Hz into 125 Hz.
pub const POLLING_RATES: [(u8, u16); 4] = [(0x01, 125), (0x02, 250), (0x03, 500), (0x04, 1000)];

pub fn polling_code_for(hz: u16) -> Result<u8> {
    POLLING_RATES
        .iter()
        .find(|(_, v)| *v == hz)
        .map(|(code, _)| *code)
        .ok_or_else(|| {
            anyhow::anyhow!("unsupported polling rate {hz} Hz (supported: 125, 250, 500, 1000)")
        })
}

pub fn polling_hz_for(code: u8) -> Option<u16> {
    POLLING_RATES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, hz)| *hz)
}

/// Maps a 0-100 percentage onto the firmware's 0-20 speed scale.
///
/// CORE computes `speed > 4 ? speed * 20 / 100 : 1`, i.e. a continuous scale with a floor of
/// 1 — never 0, because a zero rate would stall the animation. The four "speed levels" in
/// every other implementation are an invention; the firmware takes the whole range.
pub fn speed_level_from_percent(percent: u8) -> u8 {
    let percent = percent.min(100) as u16;
    if percent > 4 {
        (percent * SPEED_RAW_MAX as u16 / 100) as u8
    } else {
        1
    }
}

/// Maps a 0-100 percentage onto the firmware's 0-20 brightness scale.
///
/// CORE's rule is `brightness > 4 ? brightness * 20 / 100 : (brightness == 0 ? 0 : 1)` —
/// like speed, but 0 stays 0 so the LEDs can actually be turned off.
pub fn brightness_level_from_percent(percent: u8) -> u8 {
    let percent = percent.min(100) as u16;
    match percent {
        0 => 0,
        1..=4 => 1,
        _ => (percent * SPEED_RAW_MAX as u16 / 100) as u8,
    }
}

pub fn sanitize_debounce(ms: u8) -> u8 {
    let clamped = ms.min(DEBOUNCE_MAX_MS);
    clamped - clamped % 2
}

pub fn validate_dpi(dpi: u16) -> Result<()> {
    if !(DPI_MIN..=DPI_MAX).contains(&dpi) {
        bail!("DPI {dpi} out of range ({DPI_MIN}-{DPI_MAX})");
    }
    if !dpi.is_multiple_of(DPI_UNIT) {
        bail!("DPI {dpi} must be a multiple of {DPI_UNIT}");
    }
    Ok(())
}

pub fn encode_dpi(dpi: u16) -> u16 {
    dpi / DPI_UNIT
}

// ---------------------------------------------------------------------------
// Battery telemetry
// ---------------------------------------------------------------------------

/// HID **input** report ID carrying battery telemetry on interface 1.
///
/// Declared in the report descriptor (vendor page 0xff00, 7 bytes) but never mentioned by
/// any reference implementation, all of which report this family as having no battery
/// support. The device sends it unsolicited on a power-state change; it cannot be polled
/// (`GET_INPUT_REPORT` on this ID times out).
pub const BATTERY_REPORT_ID: u8 = 0x06;

/// Byte [1] of a battery report. Same magic as the `xx fb` configuration commands; other
/// values (e.g. the all-zero `06 00 00 …`) are idle/keepalive frames carrying no reading.
const BATTERY_MAGIC: u8 = 0xfb;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Battery {
    /// 0-100.
    pub percent: u8,
    /// True while the charging cable is connected.
    pub charging: bool,
}

/// Decodes a raw interrupt-IN report into a battery reading.
///
/// Layout, observed on a Model O 2 Wireless by watching the channel across cable
/// plug/unplug cycles:
///
/// ```text
/// 06 fb 35 00 00 00 00 00   ->  53%, discharging
/// 06 fb 35 01 00 00 00 00   ->  53%, charging
/// 06 00 00 00 00 00 00 00   ->  idle frame, no reading
/// ```
///
/// Returns `None` for anything that is not a battery-bearing frame, so callers can feed the
/// whole interrupt stream through it.
pub fn parse_battery(report: &[u8]) -> Option<Battery> {
    if report.len() < 4 || report[0] != BATTERY_REPORT_ID || report[1] != BATTERY_MAGIC {
        return None;
    }
    let percent = report[2];
    // Guard against a malformed frame being shown as a nonsense percentage.
    if percent > 100 {
        return None;
    }
    Some(Battery {
        percent,
        charging: report[3] != 0,
    })
}

// ---------------------------------------------------------------------------
// Lighting payload
// ---------------------------------------------------------------------------

/// Lighting parameters.
///
/// **The byte assignments here differ from every published reference.** CORE's own
/// `PreparePresetEffectBuffers` builds a flat `dataBuffer` and copies it into the fragments:
///
/// ```text
/// dataBuffer[0] = rate            -> packet[6]   speed
/// dataBuffer[1] = wiredBright     -> packet[7]   wired brightness
/// dataBuffer[2] = colourCount     -> packet[8]
/// dataBuffer[3] = rate            -> packet[9]   speed, duplicated
/// dataBuffer[4] = wirelessBright  -> packet[10]  wireless brightness
/// dataBuffer[5..]= colours        -> packet[11], then fragments 1 and 2
/// ```
///
/// `gloriousctl-linux` and OpenMouse call [6] "wireless brightness", [7] "wired
/// brightness", [9] "speed" and [10] a "modifier" pinned to `0x14`. Writing speed to [9]
/// leaves the real speed byte [6] stuck at maximum, which is why speed appears to do
/// nothing in every tool built on those docs.
///
/// Byte [7] looked inert during hardware bisection because it is the *wired* brightness and
/// the test ran on a receiver; byte [9] looked inert because [6] carried the same value.
/// `serde(default)` at the container level so a partial or hand-edited state file fills
/// missing fields from the factory defaults instead of failing to load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lighting {
    pub effect: Effect,
    /// Payload bytes [6] and [9]. 0-20; higher is faster.
    #[serde(default = "default_speed")]
    pub speed: u8,
    /// Payload byte [7]. 0-20. Applies only while plugged in.
    #[serde(default = "default_brightness")]
    pub brightness_wired: u8,
    /// Payload byte [10]. 0-20. Applies while on the receiver or Bluetooth.
    ///
    /// The Model O 2 declares `separateBrightness: true`, so this is genuinely independent
    /// of the wired value rather than a mirror of it.
    ///
    /// The `brightness` alias migrates state files from before the split. It maps here
    /// rather than to the wired field because [10] is the byte that was actually driving
    /// the LEDs on a receiver.
    #[serde(default = "default_brightness", alias = "brightness")]
    pub brightness_wireless: u8,
    /// Primary colour plus up to six cycle colours.
    pub colors: [Rgb; 7],
    /// Payload byte [8]. `None` means "derive from the effect".
    pub color_count: Option<u8>,
}

fn default_speed() -> u8 {
    0x0a
}

fn default_brightness() -> u8 {
    LIGHTING_MODIFIER
}

impl Default for Lighting {
    fn default() -> Self {
        Lighting {
            effect: Effect::Glorious,
            speed: default_speed(),
            brightness_wired: default_brightness(),
            brightness_wireless: default_brightness(),
            colors: DEFAULT_PALETTE,
            color_count: None,
        }
    }
}

impl Lighting {
    /// Payload byte [8]: an explicit override, else the effect's natural slot count.
    pub fn effective_color_count(&self) -> u8 {
        self.color_count
            .unwrap_or_else(|| self.effect.palette_slots() as u8)
    }

    /// Builds the three 64-byte lighting fragments (`cmd 02 fb`).
    ///
    /// Fragment 0 carries effect, speed, colour count, brightness and the primary colour;
    /// fragment 1 repeats the effect and holds palette entries 1-6; fragment 2 repeats the
    /// effect only. See the struct docs for why speed and brightness each occupy two bytes.
    pub fn fragments(&self, profile: u8) -> [[u8; PACKET_LEN]; LIGHTING_FRAGMENTS] {
        let mut frags = [[0u8; PACKET_LEN]; LIGHTING_FRAGMENTS];
        for (idx, frag) in frags.iter_mut().enumerate() {
            frag[0] = CONFIG_REPORT_ID;
            frag[1] = LIGHTING_CMD[0];
            frag[2] = LIGHTING_CMD[1];
            frag[3] = idx as u8;
            frag[4] = profile_byte(profile);
            // Every fragment repeats the effect id; the firmware rejects desynced payloads.
            frag[5] = self.effect.id();
        }

        let speed = self.speed.min(SPEED_RAW_MAX);
        let wired = self.brightness_wired.min(SPEED_RAW_MAX);
        let wireless = self.brightness_wireless.min(SPEED_RAW_MAX);

        // CORE assembles a flat `dataBuffer` and copies slices of it into the fragments:
        //   dataBuffer[0]=rate  [1]=wiredBright  [2]=colourCount  [3]=rate  [4]=wirelessBright
        //   dataBuffer[5..]    = colours, 3 bytes each
        // then fragment 0 takes dataBuffer[0..8) at byte 6, fragment 1 takes [8..17) at byte 6,
        // and fragment 2 takes [17..26) at byte 6.
        let mut data = [0u8; 26];
        data[0] = speed;
        data[1] = wired;
        data[2] = self.effective_color_count();
        data[3] = speed; // duplicated by CORE
        data[4] = wireless;
        for (i, color) in self.colors.iter().enumerate() {
            let off = 5 + i * 3;
            data[off] = color.r;
            data[off + 1] = color.g;
            data[off + 2] = color.b;
        }

        frags[0][6..14].copy_from_slice(&data[0..8]);
        frags[1][6..15].copy_from_slice(&data[8..17]);
        frags[2][6..15].copy_from_slice(&data[17..26]);

        frags
    }
}

// ---------------------------------------------------------------------------
// Settings payload
// ---------------------------------------------------------------------------

/// Stage N's `(fragment index, byte offset)` within the settings payload. Each slot holds a
/// little-endian `dpi / 50` u16 followed by the stage's RGB indicator colour.
const STAGE_SLOTS: [(usize, usize); MAX_STAGES] =
    [(0, 11), (1, 5), (1, 10), (2, 5), (2, 10), (3, 5)];

/// See [`Lighting`] for why this carries a container-level `serde(default)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Zero-indexed active DPI stage (payload byte [5]).
    pub active_stage: u8,
    /// Number of enabled stages, 4-6 (payload byte [6]).
    pub stage_count: u8,
    /// Per-stage DPI; entries past `stage_count` are written as 0.
    pub stage_dpis: [u16; MAX_STAGES],
    /// Per-stage LED indicator colour.
    pub stage_colors: [Rgb; MAX_STAGES],
    /// Lift-off distance in mm (payload byte [7]): 1 or 2.
    pub lod_mm: u8,
    /// Debounce in ms (payload byte [8]): even, 0-16.
    pub debounce_ms: u8,
    /// Polling rate wire code (payload byte [9]).
    pub polling_code: u8,
    /// Motion sync (payload byte [10]).
    ///
    /// Every other implementation writes this byte as a hardcoded `0x00`, which silently
    /// disables the feature. CORE sends `MotionSyncFlag ? 1 : 0`.
    #[serde(default)]
    pub motion_sync: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            active_stage: 0,
            stage_count: 4,
            stage_dpis: [400, 800, 1600, 3200, 0, 0],
            stage_colors: DEFAULT_STAGE_COLORS,
            lod_mm: LOD_MEDIUM_MM,
            debounce_ms: 10,
            polling_code: polling_code_for(1000).unwrap(),
            motion_sync: false,
        }
    }
}

impl Settings {
    pub fn polling_hz(&self) -> u16 {
        polling_hz_for(self.polling_code).unwrap_or(1000)
    }

    pub fn active_dpi(&self) -> u16 {
        self.stage_dpis
            .get(self.active_stage as usize)
            .copied()
            .unwrap_or(800)
    }

    /// Builds the four 64-byte settings fragments (`cmd 04 fb`).
    ///
    /// Fragment 0 carries the global bytes (active stage, stage count, LOD, debounce,
    /// polling) plus stage 1; stages 2-3 live in fragment 1, stages 4-5 in fragment 2,
    /// and stage 6 in fragment 3.
    pub fn fragments(&self, profile: u8) -> [[u8; PACKET_LEN]; SETTINGS_FRAGMENTS] {
        let mut frags = [[0u8; PACKET_LEN]; SETTINGS_FRAGMENTS];
        for (idx, frag) in frags.iter_mut().enumerate() {
            frag[0] = CONFIG_REPORT_ID;
            frag[1] = SETTINGS_CMD[0];
            frag[2] = SETTINGS_CMD[1];
            frag[3] = idx as u8;
            frag[4] = profile_byte(profile);
        }

        let first = &mut frags[0];
        first[5] = self.active_stage;
        first[6] = self.stage_count;
        first[7] = self.lod_mm;
        first[8] = self.debounce_ms;
        first[9] = self.polling_code;
        first[10] = self.motion_sync as u8;

        for (stage, (frag_idx, off)) in STAGE_SLOTS.iter().enumerate() {
            // Stages beyond the enabled count are zeroed, matching the stock payload.
            let dpi = if stage < self.stage_count as usize {
                encode_dpi(self.stage_dpis[stage])
            } else {
                0
            };
            let frag = &mut frags[*frag_idx];
            frag[*off] = (dpi & 0xff) as u8;
            frag[*off + 1] = (dpi >> 8) as u8;
            let color = self.stage_colors[stage];
            frag[*off + 2] = color.r;
            frag[*off + 3] = color.g;
            frag[*off + 4] = color.b;
        }

        frags
    }
}

/// Renders fragments the way `--trace` prints them.
pub fn trace_fragments(label: &str, frags: &[&[u8]]) -> String {
    let mut out = format!("{label}: {} fragments\n", frags.len());
    for (i, frag) in frags.iter().enumerate() {
        out.push_str(&format!("  [{i}] "));
        for (j, byte) in frag.iter().enumerate() {
            if j > 0 && j % 16 == 0 {
                out.push_str("\n      ");
            }
            out.push_str(&format!("{byte:02x} "));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lighting_header_matches_capture() {
        let frags = Lighting::default().fragments(0);
        for (idx, frag) in frags.iter().enumerate() {
            assert_eq!(&frag[0..5], &[0x03, 0x02, 0xfb, idx as u8, 0x01]);
            assert_eq!(frag[5], Effect::Glorious.id(), "effect id repeats");
        }
    }

    #[test]
    fn lighting_body_places_every_field() {
        let lighting = Lighting {
            effect: Effect::Breathing,
            speed: 5,
            brightness_wired: 15,
            brightness_wireless: 15,
            ..Lighting::default()
        };
        let frags = lighting.fragments(0);
        assert_eq!(frags[0][8], 7, "breathing cycles all seven slots");
        // Colour 0 in fragment 0 at [11]; colours 1-3 in fragment 1 at [6..15);
        // colours 4-6 in fragment 2 at [6..15). This split is CORE's, not contiguous.
        assert_eq!(&frags[0][11..14], &[0xff, 0x00, 0x00], "colour 0");
        assert_eq!(&frags[1][6..9], &[0xff, 0x7f, 0x00], "colour 1");
        assert_eq!(&frags[1][12..15], &[0x00, 0xff, 0x00], "colour 3");
        assert_eq!(
            &frags[2][6..9],
            &[0x00, 0x00, 0xff],
            "colour 4 starts fragment 2"
        );
        assert_eq!(&frags[2][12..15], &[0x94, 0x00, 0xd3], "colour 6");
    }

    #[test]
    fn colours_four_to_six_live_in_fragment_two() {
        // Regression: these used to be packed contiguously into fragment 1, which put them
        // at offsets the firmware reads as something else entirely.
        let frags = Lighting::default().fragments(0);
        assert_eq!(
            &frags[1][15..24],
            &[0u8; 9],
            "fragment 1 must end after colour 3"
        );
        assert_ne!(
            &frags[2][6..15],
            &[0u8; 9],
            "fragment 2 carries colours 4-6"
        );
    }

    #[test]
    fn lighting_scalars_land_on_cores_offsets() {
        // The central finding. Published references put speed on [9] and brightness on
        // [6]/[7], leaving the real speed byte [6] pinned at max — which is why speed
        // appears to do nothing in every tool built on them.
        let frags = Lighting {
            speed: 5,
            brightness_wired: 15,
            brightness_wireless: 9,
            ..Lighting::default()
        }
        .fragments(0);
        assert_eq!(frags[0][6], 5, "[6] speed");
        assert_eq!(frags[0][7], 15, "[7] wired brightness");
        assert_eq!(frags[0][9], 5, "[9] speed, duplicated by CORE");
        assert_eq!(frags[0][10], 9, "[10] wireless brightness");
    }

    #[test]
    fn wired_and_wireless_brightness_are_independent() {
        // This model reports separateBrightness: true, so [7] and [10] must not be mirrored.
        let frags = Lighting {
            brightness_wired: 20,
            brightness_wireless: 0,
            ..Lighting::default()
        }
        .fragments(0);
        assert_eq!(frags[0][7], 20);
        assert_eq!(frags[0][10], 0);
    }

    #[test]
    fn lighting_values_are_clamped_to_the_hardware_range() {
        let frags = Lighting {
            speed: 0xff,
            brightness_wired: 0xff,
            brightness_wireless: 0xff,
            ..Lighting::default()
        }
        .fragments(0);
        for offset in [6, 7, 9, 10] {
            assert_eq!(frags[0][offset], SPEED_RAW_MAX, "byte [{offset}] clamped");
        }
    }

    #[test]
    fn palette_is_not_silently_black() {
        // Regression: multi-colour effects used to declare 7 slots but send 6 black colours.
        let frags = Lighting::default().fragments(0);
        let sent: Vec<u8> = (1..7)
            .flat_map(|i| frags[1][3 + i * 3..6 + i * 3].to_vec())
            .collect();
        assert!(
            sent.iter().any(|b| *b != 0),
            "palette slots 1-6 must be populated"
        );
    }

    #[test]
    fn solid_uses_one_slot_and_primary_color() {
        let mut lighting = Lighting {
            effect: Effect::Solid,
            ..Lighting::default()
        };
        lighting.colors[0] = Rgb::new(0x12, 0x34, 0x56);
        let frags = lighting.fragments(0);
        assert_eq!(frags[0][8], 1);
        assert_eq!(&frags[0][11..14], &[0x12, 0x34, 0x56]);
    }

    #[test]
    fn settings_header_and_globals() {
        let settings = Settings {
            active_stage: 2,
            stage_count: 5,
            lod_mm: LOD_HIGH_MM,
            debounce_ms: 8,
            polling_code: polling_code_for(1000).unwrap(),
            motion_sync: true,
            ..Settings::default()
        };
        let frags = settings.fragments(0);
        for (idx, frag) in frags.iter().enumerate() {
            assert_eq!(&frag[0..5], &[0x03, 0x04, 0xfb, idx as u8, 0x01]);
        }
        // active stage, stage count, LOD, debounce, polling, motion sync
        assert_eq!(&frags[0][5..11], &[2, 5, 2, 8, 0x04, 0x01]);
    }

    #[test]
    fn polling_codes_match_core_not_the_third_party_docs() {
        // CORE indexes [125,250,500,1000] into [1,2,3,4]. gloriousctl-linux documents a
        // rotation of this, under which asking for 1000 Hz actually selects 125 Hz.
        assert_eq!(polling_code_for(125).unwrap(), 1);
        assert_eq!(polling_code_for(250).unwrap(), 2);
        assert_eq!(polling_code_for(500).unwrap(), 3);
        assert_eq!(polling_code_for(1000).unwrap(), 4);
    }

    #[test]
    fn motion_sync_occupies_byte_ten() {
        // Every other implementation hardcodes this to 0, silently disabling the feature.
        let off = Settings {
            motion_sync: false,
            ..Settings::default()
        };
        assert_eq!(off.fragments(0)[0][10], 0);
        let on = Settings {
            motion_sync: true,
            ..Settings::default()
        };
        assert_eq!(on.fragments(0)[0][10], 1);
    }

    #[test]
    fn settings_encode_dpi_little_endian_over_50() {
        let settings = Settings {
            stage_dpis: [400, 800, 1600, 3200, 0, 0],
            ..Settings::default()
        };
        let frags = settings.fragments(0);
        // 400/50 = 8 in fragment 0 at offset 11, then the stage colour.
        assert_eq!(&frags[0][11..16], &[8, 0, 0xff, 0x00, 0x00]);
        // 800/50 = 16 in fragment 1 at offset 5.
        assert_eq!(&frags[1][5..10], &[16, 0, 0x00, 0x00, 0xff]);
        // 3200/50 = 64 in fragment 2 at offset 5.
        assert_eq!(&frags[2][5..10], &[64, 0, 0xff, 0xff, 0x00]);
    }

    #[test]
    fn high_dpi_spans_both_bytes() {
        let mut settings = Settings::default();
        settings.stage_dpis[0] = 26_000; // 26000/50 = 520 = 0x0208
        let frags = settings.fragments(0);
        assert_eq!(&frags[0][11..13], &[0x08, 0x02]);
    }

    #[test]
    fn stages_past_the_count_are_zeroed() {
        let mut settings = Settings {
            stage_count: 4,
            ..Settings::default()
        };
        settings.stage_dpis[4] = 5000;
        let frags = settings.fragments(0);
        assert_eq!(&frags[2][10..12], &[0, 0], "stage 5 is disabled");
    }

    #[test]
    fn debounce_rounds_down_to_even_and_clamps() {
        assert_eq!(sanitize_debounce(7), 6);
        assert_eq!(sanitize_debounce(8), 8);
        assert_eq!(sanitize_debounce(200), 16);
        assert_eq!(sanitize_debounce(0), 0);
    }

    #[test]
    fn percent_maps_match_cores_formula() {
        // CORE: speed > 4 ? speed * 20 / 100 : 1 - continuous, floored at 1 so an animated
        // effect never stalls.
        assert_eq!(speed_level_from_percent(100), 20);
        assert_eq!(speed_level_from_percent(50), 10);
        assert_eq!(speed_level_from_percent(0), 1, "speed never reaches 0");
        assert_eq!(speed_level_from_percent(200), 20, "clamps above 100");

        // CORE: brightness > 4 ? brightness * 20 / 100 : (brightness == 0 ? 0 : 1).
        assert_eq!(brightness_level_from_percent(100), 20);
        assert_eq!(brightness_level_from_percent(50), 10);
        assert_eq!(brightness_level_from_percent(0), 0, "0% turns LEDs off");
        assert_eq!(brightness_level_from_percent(3), 1);
    }

    #[test]
    fn intermediate_percentages_are_not_snapped_to_levels() {
        // The 4/5-level tables every other implementation uses are an invention.
        assert_eq!(brightness_level_from_percent(40), 8);
        assert_eq!(speed_level_from_percent(35), 7);
    }

    #[test]
    fn polling_round_trips() {
        for (code, hz) in POLLING_RATES {
            assert_eq!(polling_code_for(hz).unwrap(), code);
            assert_eq!(polling_hz_for(code).unwrap(), hz);
        }
        assert!(polling_code_for(8000).is_err());
    }

    #[test]
    fn dpi_validation() {
        assert!(validate_dpi(800).is_ok());
        assert!(validate_dpi(26_000).is_ok());
        assert!(validate_dpi(825).is_err(), "not a multiple of 50");
        assert!(validate_dpi(50).is_err(), "below minimum");
    }

    #[test]
    fn battery_frames_decode() {
        assert_eq!(
            parse_battery(&[0x06, 0xfb, 0x35, 0x00, 0, 0, 0, 0]),
            Some(Battery {
                percent: 53,
                charging: false
            })
        );
        assert_eq!(
            parse_battery(&[0x06, 0xfb, 0x35, 0x01, 0, 0, 0, 0]),
            Some(Battery {
                percent: 53,
                charging: true
            })
        );
        assert_eq!(
            parse_battery(&[0x06, 0xfb, 0x64, 0x01, 0, 0, 0, 0]),
            Some(Battery {
                percent: 100,
                charging: true
            })
        );
    }

    #[test]
    fn non_battery_frames_are_rejected() {
        // Idle keepalive on the same report id.
        assert_eq!(parse_battery(&[0x06, 0x00, 0x00, 0x00, 0, 0, 0, 0]), None);
        // Ordinary movement report.
        assert_eq!(parse_battery(&[0x01, 0x00, 0x01, 0x00, 0, 0, 0, 0]), None);
        // Truncated.
        assert_eq!(parse_battery(&[0x06, 0xfb]), None);
        // Out-of-range percentage must not be surfaced as a bogus reading.
        assert_eq!(parse_battery(&[0x06, 0xfb, 0xff, 0x00, 0, 0, 0, 0]), None);
    }

    #[test]
    fn color_parsing() {
        assert_eq!("#ff0000".parse::<Rgb>().unwrap(), Rgb::new(255, 0, 0));
        assert_eq!("00ff00".parse::<Rgb>().unwrap(), Rgb::new(0, 255, 0));
        assert_eq!("#f00".parse::<Rgb>().unwrap(), Rgb::new(255, 0, 0));
        assert_eq!(Rgb::new(0x12, 0x34, 0x56).to_string(), "#123456");
        assert!("nope".parse::<Rgb>().is_err());
    }

    #[test]
    fn effect_aliases_resolve() {
        assert_eq!(
            "cycle".parse::<Effect>().unwrap(),
            Effect::SeamlessBreathing
        );
        assert_eq!("pulse".parse::<Effect>().unwrap(), Effect::Breathing);
        assert_eq!(
            "pulse-one".parse::<Effect>().unwrap(),
            Effect::BreathingSingle
        );
        assert_eq!("GLORIOUS".parse::<Effect>().unwrap(), Effect::Glorious);
        assert!("nope".parse::<Effect>().is_err());
    }
}
