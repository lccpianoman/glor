//! Button bindings (`cmd 03 fb`) and macros (`cmd 05 fb`).
//!
//! Layout taken from Glorious CORE 2.1.21 (`PrepareKeybindingBuffers2`,
//! `PrepareMacroBuffers`). Each button occupies **4 bytes** in a flat key buffer indexed by
//! `hidButtonID * 4`; the receiver transmits the first 24 bytes as two 12-byte fragments,
//! which is exactly the Model O 2's six configurable buttons.
//!
//! See `PROTOCOL.md` for the full table.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::protocol::{CONFIG_REPORT_ID, PACKET_LEN};

const BUTTONS_CMD: [u8; 2] = [0x03, 0xfb];
const MACRO_CMD: [u8; 2] = [0x05, 0xfb];

/// Bytes per button slot in the key buffer.
const SLOT_LEN: usize = 4;
/// Buttons the Model O 2 exposes: left, right, middle, back, forward, DPI cycle.
pub const BUTTON_COUNT: usize = 6;
/// Payload bytes per fragment, and where they start.
const CHUNK_LEN: usize = 12;
const CHUNK_OFFSET: usize = 5;
/// Macro events per page over the receiver, and where the event data starts.
const MACRO_EVENTS_PER_PAGE: usize = 3;
const MACRO_DATA_OFFSET: usize = 7;

// ---------------------------------------------------------------------------
// Binding type bytes (slot byte [0])
// ---------------------------------------------------------------------------

const TYPE_DISABLED: u8 = 0;
const TYPE_MOUSE: u8 = 1;
const TYPE_KEYSTROKE: u8 = 2;
const TYPE_MULTIMEDIA: u8 = 3;
const TYPE_DPI: u8 = 102;
const TYPE_BATTERY_CHECK: u8 = 119;
const TYPE_LAYER_SHIFT: u8 = 136;
/// Macro slots are encoded as `31 + macroId`.
const TYPE_MACRO_BASE: u8 = 31;

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

/// The physical buttons, in `hidButtonID` order — which is also key-buffer slot order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Button {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    DpiCycle,
}

impl Button {
    pub const ALL: [Button; BUTTON_COUNT] = [
        Button::Left,
        Button::Right,
        Button::Middle,
        Button::Back,
        Button::Forward,
        Button::DpiCycle,
    ];

    /// `hidButtonID` from CORE's `DeviceButtonMapping`.
    pub fn slot(self) -> usize {
        match self {
            Button::Left => 0,
            Button::Right => 1,
            Button::Middle => 2,
            Button::Back => 3,
            Button::Forward => 4,
            Button::DpiCycle => 5,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Button::Left => "left",
            Button::Right => "right",
            Button::Middle => "middle",
            Button::Back => "back",
            Button::Forward => "forward",
            Button::DpiCycle => "dpi",
        }
    }
}

impl FromStr for Button {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        let key = s.trim().to_ascii_lowercase();
        let key = match key.as_str() {
            "dpi-cycle" | "dpicycle" => "dpi",
            "mouse4" | "side1" => "back",
            "mouse5" | "side2" => "forward",
            "wheel" | "mmb" => "middle",
            other => other,
        };
        Button::ALL
            .into_iter()
            .find(|b| b.name() == key)
            .ok_or_else(|| anyhow::anyhow!("unknown button '{s}'"))
    }
}

/// Plain mouse actions (slot type `1`), valued by CORE's `MouseFunctionHIDMap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MouseAction {
    LeftClick,
    RightClick,
    MiddleClick,
    Back,
    Forward,
    ScrollUp,
    ScrollDown,
    ProfileCycleUp,
    ProfileCycleDown,
}

impl MouseAction {
    fn id(self) -> u8 {
        match self {
            MouseAction::LeftClick => 1,
            MouseAction::RightClick => 2,
            MouseAction::MiddleClick => 3,
            MouseAction::Back => 4,
            MouseAction::Forward => 5,
            MouseAction::ScrollUp => 160,
            MouseAction::ScrollDown => 161,
            MouseAction::ProfileCycleUp => 176,
            MouseAction::ProfileCycleDown => 177,
        }
    }

    fn name(self) -> &'static str {
        match self {
            MouseAction::LeftClick => "left-click",
            MouseAction::RightClick => "right-click",
            MouseAction::MiddleClick => "middle-click",
            MouseAction::Back => "back",
            MouseAction::Forward => "forward",
            MouseAction::ScrollUp => "scroll-up",
            MouseAction::ScrollDown => "scroll-down",
            MouseAction::ProfileCycleUp => "profile-up",
            MouseAction::ProfileCycleDown => "profile-down",
        }
    }

    const ALL: [MouseAction; 9] = [
        MouseAction::LeftClick,
        MouseAction::RightClick,
        MouseAction::MiddleClick,
        MouseAction::Back,
        MouseAction::Forward,
        MouseAction::ScrollUp,
        MouseAction::ScrollDown,
        MouseAction::ProfileCycleUp,
        MouseAction::ProfileCycleDown,
    ];
}

/// DPI actions, which use slot type `102` rather than `1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DpiAction {
    StageUp,
    StageDown,
    CycleUp,
    CycleDown,
    /// Holds a temporary DPI for as long as the button is held.
    Shift(u16),
}

impl DpiAction {
    fn id(self) -> u8 {
        match self {
            DpiAction::StageUp => 1,
            DpiAction::StageDown => 2,
            DpiAction::CycleUp => 3,
            DpiAction::CycleDown => 4,
            DpiAction::Shift(_) => 5,
        }
    }
}

/// Consumer-page actions (slot type `3`). Values are CORE's `MediaHIDMap`, and are written
/// into the slot **backwards** — `slot[2] = map[0]`, `slot[1] = map[1]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MediaAction {
    PlayPause,
    Stop,
    Mute,
    VolumeUp,
    VolumeDown,
    NextTrack,
    PreviousTrack,
    OpenMediaPlayer,
}

impl MediaAction {
    fn map(self) -> [u8; 2] {
        match self {
            MediaAction::OpenMediaPlayer => [1, 131],
            MediaAction::PlayPause => [0, 205],
            MediaAction::Stop => [0, 183],
            MediaAction::Mute => [0, 226],
            MediaAction::VolumeUp => [0, 233],
            MediaAction::VolumeDown => [0, 234],
            MediaAction::NextTrack => [0, 181],
            MediaAction::PreviousTrack => [0, 182],
        }
    }

    fn name(self) -> &'static str {
        match self {
            MediaAction::PlayPause => "play-pause",
            MediaAction::Stop => "stop",
            MediaAction::Mute => "mute",
            MediaAction::VolumeUp => "volume-up",
            MediaAction::VolumeDown => "volume-down",
            MediaAction::NextTrack => "next-track",
            MediaAction::PreviousTrack => "previous-track",
            MediaAction::OpenMediaPlayer => "media-player",
        }
    }

    const ALL: [MediaAction; 8] = [
        MediaAction::PlayPause,
        MediaAction::Stop,
        MediaAction::Mute,
        MediaAction::VolumeUp,
        MediaAction::VolumeDown,
        MediaAction::NextTrack,
        MediaAction::PreviousTrack,
        MediaAction::OpenMediaPlayer,
    ];
}

/// How a macro repeats. `Toggle` is the "press once to start, again to stop" behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MacroMode {
    #[default]
    Once,
    WhileHeld,
    Toggle,
}

impl MacroMode {
    fn id(self) -> u8 {
        match self {
            MacroMode::Once => 1,
            MacroMode::WhileHeld => 224,
            MacroMode::Toggle => 225,
        }
    }
}

/// Keyboard modifier bitmask: bit0 Ctrl, bit1 Shift, bit2 Alt, bit3 Meta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Modifiers {
    pub fn mask(self) -> u8 {
        (self.ctrl as u8) | (self.shift as u8) << 1 | (self.alt as u8) << 2 | (self.meta as u8) << 3
    }

    /// Parses a `ctrl+shift+` style prefix, returning the mask and the remaining key name.
    pub fn parse_prefixed(spec: &str) -> (Modifiers, &str) {
        let mut mods = Modifiers::default();
        let mut rest = spec;
        loop {
            let Some((head, tail)) = rest.split_once('+') else {
                return (mods, rest);
            };
            match head.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => mods.ctrl = true,
                "shift" => mods.shift = true,
                "alt" => mods.alt = true,
                "meta" | "super" | "win" | "cmd" => mods.meta = true,
                // Not a modifier, so `head` is the key and the '+' belonged to it.
                _ => return (mods, rest),
            }
            rest = tail;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Binding {
    Disabled,
    Mouse(MouseAction),
    Dpi(DpiAction),
    Media(MediaAction),
    Key { modifiers: Modifiers, code: u8 },
    Macro { id: u8, mode: MacroMode },
    BatteryCheck,
    LayerShift,
}

impl Binding {
    /// Encodes the 4-byte slot.
    fn encode(self) -> [u8; SLOT_LEN] {
        match self {
            Binding::Disabled => [TYPE_DISABLED, 0, 0, 0],
            Binding::Mouse(action) => [TYPE_MOUSE, action.id(), 0, 0],
            Binding::Dpi(action) => {
                let (lo, hi) = match action {
                    // CORE encodes the shift DPI as dpi/50, little-endian, in [2] and [3].
                    DpiAction::Shift(dpi) => {
                        let encoded = dpi / 50;
                        ((encoded & 0xff) as u8, (encoded >> 8) as u8)
                    }
                    _ => (0, 0),
                };
                [TYPE_DPI, action.id(), lo, hi]
            }
            // Written in reverse: slot[2] = map[0], slot[1] = map[1].
            Binding::Media(action) => {
                let map = action.map();
                [TYPE_MULTIMEDIA, map[1], map[0], 0]
            }
            Binding::Key { modifiers, code } => [TYPE_KEYSTROKE, modifiers.mask(), code, 0],
            Binding::Macro { id, mode } => [TYPE_MACRO_BASE.wrapping_add(id), mode.id(), 0, 0],
            Binding::BatteryCheck => [TYPE_BATTERY_CHECK, 0, 0, 0],
            Binding::LayerShift => [TYPE_LAYER_SHIFT, 0, 0, 0],
        }
    }

    pub fn describe(self) -> String {
        match self {
            Binding::Disabled => "disabled".into(),
            Binding::Mouse(a) => a.name().into(),
            Binding::Dpi(DpiAction::Shift(dpi)) => format!("dpi-shift {dpi}"),
            Binding::Dpi(DpiAction::StageUp) => "dpi-stage-up".into(),
            Binding::Dpi(DpiAction::StageDown) => "dpi-stage-down".into(),
            Binding::Dpi(DpiAction::CycleUp) => "dpi-cycle-up".into(),
            Binding::Dpi(DpiAction::CycleDown) => "dpi-cycle-down".into(),
            Binding::Media(a) => a.name().into(),
            Binding::Key { modifiers, code } => {
                let mut parts = Vec::new();
                if modifiers.ctrl {
                    parts.push("ctrl");
                }
                if modifiers.shift {
                    parts.push("shift");
                }
                if modifiers.alt {
                    parts.push("alt");
                }
                if modifiers.meta {
                    parts.push("meta");
                }
                let key = key_name(code).unwrap_or("?");
                if parts.is_empty() {
                    format!("key {key}")
                } else {
                    format!("key {}+{key}", parts.join("+"))
                }
            }
            Binding::Macro { id, mode } => format!("macro {id} ({mode:?})"),
            Binding::BatteryCheck => "battery-check".into(),
            Binding::LayerShift => "layer-shift".into(),
        }
    }
}

impl FromStr for Binding {
    type Err = anyhow::Error;

    /// Accepts `disabled`, a mouse action, a media action, `dpi-shift:1600`,
    /// `macro:2` / `macro:2:toggle`, `battery-check`, `layer-shift`, or `key:ctrl+c`.
    fn from_str(s: &str) -> Result<Self> {
        let spec = s.trim();
        let lower = spec.to_ascii_lowercase();

        if let Some(rest) = lower.strip_prefix("key:") {
            let (modifiers, key) = Modifiers::parse_prefixed(rest);
            let code = key_code(key)
                .ok_or_else(|| anyhow::anyhow!("unknown key '{key}' in binding '{s}'"))?;
            return Ok(Binding::Key { modifiers, code });
        }
        if let Some(rest) = lower.strip_prefix("dpi-shift:") {
            let dpi: u16 = rest
                .parse()
                .map_err(|_| anyhow::anyhow!("bad DPI '{rest}'"))?;
            crate::protocol::validate_dpi(dpi)?;
            return Ok(Binding::Dpi(DpiAction::Shift(dpi)));
        }
        if let Some(rest) = lower.strip_prefix("macro:") {
            let mut parts = rest.split(':');
            let id: u8 = parts
                .next()
                .unwrap_or_default()
                .parse()
                .map_err(|_| anyhow::anyhow!("bad macro id in '{s}'"))?;
            let mode = match parts.next() {
                None | Some("once") => MacroMode::Once,
                Some("hold") | Some("while-held") => MacroMode::WhileHeld,
                Some("toggle") => MacroMode::Toggle,
                Some(other) => bail!("unknown macro mode '{other}' (once, hold, toggle)"),
            };
            return Ok(Binding::Macro { id, mode });
        }

        match lower.as_str() {
            "disabled" | "none" | "off" => return Ok(Binding::Disabled),
            "battery-check" | "battery" => return Ok(Binding::BatteryCheck),
            "layer-shift" => return Ok(Binding::LayerShift),
            "dpi-stage-up" => return Ok(Binding::Dpi(DpiAction::StageUp)),
            "dpi-stage-down" => return Ok(Binding::Dpi(DpiAction::StageDown)),
            "dpi-cycle-up" | "dpi" => return Ok(Binding::Dpi(DpiAction::CycleUp)),
            "dpi-cycle-down" => return Ok(Binding::Dpi(DpiAction::CycleDown)),
            _ => {}
        }
        if let Some(a) = MouseAction::ALL.into_iter().find(|a| a.name() == lower) {
            return Ok(Binding::Mouse(a));
        }
        if let Some(a) = MediaAction::ALL.into_iter().find(|a| a.name() == lower) {
            return Ok(Binding::Media(a));
        }
        bail!("unknown binding '{s}' (see `glor bind --help`)")
    }
}

/// The six button bindings of one profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Buttons {
    pub slots: [Binding; BUTTON_COUNT],
}

impl Default for Buttons {
    /// CORE's `DEFAULT_KEY_BUFFER`, which is the factory mapping.
    fn default() -> Self {
        Buttons {
            slots: [
                Binding::Mouse(MouseAction::LeftClick),
                Binding::Mouse(MouseAction::RightClick),
                Binding::Mouse(MouseAction::MiddleClick),
                Binding::Mouse(MouseAction::Back),
                Binding::Mouse(MouseAction::Forward),
                Binding::Dpi(DpiAction::CycleUp),
            ],
        }
    }
}

impl Buttons {
    pub fn get(&self, button: Button) -> Binding {
        self.slots[button.slot()]
    }

    pub fn set(&mut self, button: Button, binding: Binding) {
        self.slots[button.slot()] = binding;
    }

    /// Refuses a mapping that would leave no way to left-click.
    ///
    /// The firmware happily accepts it, and the result is a mouse you cannot use to fix the
    /// mistake without the hardware factory reset.
    pub fn validate(&self) -> Result<()> {
        let has_left = self
            .slots
            .iter()
            .any(|b| matches!(b, Binding::Mouse(MouseAction::LeftClick)));
        if !has_left {
            bail!(
                "this mapping leaves no button bound to left-click, which would make the \
                 mouse unusable. Bind left-click somewhere first, or pass --allow-no-left-click."
            );
        }
        Ok(())
    }

    /// The flat 24-byte key buffer the receiver transmits.
    fn key_buffer(&self) -> [u8; BUTTON_COUNT * SLOT_LEN] {
        let mut buf = [0u8; BUTTON_COUNT * SLOT_LEN];
        for (idx, binding) in self.slots.iter().enumerate() {
            let off = idx * SLOT_LEN;
            buf[off..off + SLOT_LEN].copy_from_slice(&binding.encode());
        }
        buf
    }

    /// Builds the two 64-byte button fragments (`cmd 03 fb`).
    pub fn fragments(&self, profile: u8) -> [[u8; PACKET_LEN]; 2] {
        let data = self.key_buffer();
        let mut frags = [[0u8; PACKET_LEN]; 2];
        for (idx, frag) in frags.iter_mut().enumerate() {
            frag[0] = CONFIG_REPORT_ID;
            frag[1] = BUTTONS_CMD[0];
            frag[2] = BUTTONS_CMD[1];
            frag[3] = idx as u8;
            frag[4] = profile.min(crate::protocol::PROFILE_COUNT - 1) + 1;
            let start = idx * CHUNK_LEN;
            frag[CHUNK_OFFSET..CHUNK_OFFSET + CHUNK_LEN]
                .copy_from_slice(&data[start..start + CHUNK_LEN]);
        }
        frags
    }
}

// ---------------------------------------------------------------------------
// Macros
// ---------------------------------------------------------------------------

/// One macro step: a key press or release, plus the delay until the next step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroEvent {
    pub code: u8,
    pub press: bool,
    /// Milliseconds until the next event. CORE floors this at 1.
    pub delay_ms: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Macro {
    pub events: Vec<MacroEvent>,
}

impl Macro {
    /// Builds the macro pages (`cmd 05 fb`), 3 events per page over the receiver.
    ///
    /// Header: `[03, 05, fb, macroId, pageIndex, pageCount, eventCount]`, data at offset 7.
    /// Each event is `[press<<7 | delay>>8, delay & 0xff, keyCode]`.
    pub fn fragments(&self, macro_id: u8) -> Vec<[u8; PACKET_LEN]> {
        let count = self.events.len();
        let pages = count.div_ceil(MACRO_EVENTS_PER_PAGE).max(1);
        let mut out = Vec::with_capacity(pages);

        for page in 0..pages {
            let mut frag = [0u8; PACKET_LEN];
            frag[0] = CONFIG_REPORT_ID;
            frag[1] = MACRO_CMD[0];
            frag[2] = MACRO_CMD[1];
            frag[3] = macro_id;
            frag[4] = page as u8;
            frag[5] = pages as u8;
            frag[6] = count.min(u8::MAX as usize) as u8;

            for slot in 0..MACRO_EVENTS_PER_PAGE {
                let Some(event) = self.events.get(page * MACRO_EVENTS_PER_PAGE + slot) else {
                    break;
                };
                let delay = event.delay_ms.max(1);
                let off = MACRO_DATA_OFFSET + slot * 3;
                frag[off] = (event.press as u8) << 7 | (delay >> 8) as u8;
                frag[off + 1] = (delay & 0xff) as u8;
                frag[off + 2] = event.code;
            }
            out.push(frag);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// HID keyboard usage codes
// ---------------------------------------------------------------------------

/// Standard USB HID keyboard usage IDs. CORE uses the same table.
const KEYS: &[(&str, u8)] = &[
    ("a", 4),
    ("b", 5),
    ("c", 6),
    ("d", 7),
    ("e", 8),
    ("f", 9),
    ("g", 10),
    ("h", 11),
    ("i", 12),
    ("j", 13),
    ("k", 14),
    ("l", 15),
    ("m", 16),
    ("n", 17),
    ("o", 18),
    ("p", 19),
    ("q", 20),
    ("r", 21),
    ("s", 22),
    ("t", 23),
    ("u", 24),
    ("v", 25),
    ("w", 26),
    ("x", 27),
    ("y", 28),
    ("z", 29),
    ("1", 30),
    ("2", 31),
    ("3", 32),
    ("4", 33),
    ("5", 34),
    ("6", 35),
    ("7", 36),
    ("8", 37),
    ("9", 38),
    ("0", 39),
    ("enter", 40),
    ("escape", 41),
    ("backspace", 42),
    ("tab", 43),
    ("space", 44),
    ("minus", 45),
    ("equal", 46),
    ("leftbracket", 47),
    ("rightbracket", 48),
    ("backslash", 49),
    ("semicolon", 51),
    ("quote", 52),
    ("grave", 53),
    ("comma", 54),
    ("period", 55),
    ("slash", 56),
    ("capslock", 57),
    ("f1", 58),
    ("f2", 59),
    ("f3", 60),
    ("f4", 61),
    ("f5", 62),
    ("f6", 63),
    ("f7", 64),
    ("f8", 65),
    ("f9", 66),
    ("f10", 67),
    ("f11", 68),
    ("f12", 69),
    ("printscreen", 70),
    ("scrolllock", 71),
    ("pause", 72),
    ("insert", 73),
    ("home", 74),
    ("pageup", 75),
    ("delete", 76),
    ("end", 77),
    ("pagedown", 78),
    ("right", 79),
    ("left", 80),
    ("down", 81),
    ("up", 82),
    ("ctrl", 224),
    ("shift", 225),
    ("alt", 226),
    ("meta", 227),
];

pub fn key_code(name: &str) -> Option<u8> {
    let key = name.trim().to_ascii_lowercase();
    KEYS.iter().find(|(n, _)| *n == key).map(|(_, c)| *c)
}

pub fn key_name(code: u8) -> Option<&'static str> {
    KEYS.iter().find(|(_, c)| *c == code).map(|(n, _)| *n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mapping_matches_cores_key_buffer() {
        // CORE's DEFAULT_KEY_BUFFER, first six slots.
        let expected: [u8; 24] = [
            1, 1, 0, 0, // left
            1, 2, 0, 0, // right
            1, 3, 0, 0, // middle
            1, 4, 0, 0, // back
            1, 5, 0, 0, // forward
            102, 3, 0, 0, // DPI cycle up
        ];
        assert_eq!(Buttons::default().key_buffer(), expected);
    }

    #[test]
    fn fragments_split_twelve_bytes_each_at_offset_five() {
        let frags = Buttons::default().fragments(0);
        assert_eq!(&frags[0][0..5], &[0x03, 0x03, 0xfb, 0x00, 0x01]);
        assert_eq!(&frags[1][0..5], &[0x03, 0x03, 0xfb, 0x01, 0x01]);
        assert_eq!(&frags[0][5..17], &[1, 1, 0, 0, 1, 2, 0, 0, 1, 3, 0, 0]);
        assert_eq!(&frags[1][5..17], &[1, 4, 0, 0, 1, 5, 0, 0, 102, 3, 0, 0]);
        // Nothing may spill past the 12-byte chunk.
        assert_eq!(&frags[0][17..24], &[0u8; 7]);
    }

    #[test]
    fn fragments_carry_the_profile_index() {
        let frags = Buttons::default().fragments(2);
        assert_eq!(frags[0][4], 3, "profileIndex + 1");
        assert_eq!(frags[1][4], 3);
    }

    #[test]
    fn binding_encodings_match_core() {
        assert_eq!(Binding::Disabled.encode(), [0, 0, 0, 0]);
        assert_eq!(Binding::BatteryCheck.encode(), [119, 0, 0, 0]);
        assert_eq!(Binding::LayerShift.encode(), [136, 0, 0, 0]);
        assert_eq!(
            Binding::Mouse(MouseAction::ScrollUp).encode(),
            [1, 160, 0, 0]
        );
        // Macro slots are 31 + id, with the repeat mode in byte 1.
        assert_eq!(
            Binding::Macro {
                id: 1,
                mode: MacroMode::Toggle
            }
            .encode(),
            [32, 225, 0, 0]
        );
        assert_eq!(
            Binding::Macro {
                id: 0,
                mode: MacroMode::WhileHeld
            }
            .encode(),
            [31, 224, 0, 0]
        );
    }

    #[test]
    fn media_bindings_are_written_backwards() {
        // CORE does `slot[dataOffset + 2 - i] = hidMap[i]`, so the pair lands reversed.
        assert_eq!(
            Binding::Media(MediaAction::PlayPause).encode(),
            [3, 205, 0, 0]
        );
        assert_eq!(
            Binding::Media(MediaAction::OpenMediaPlayer).encode(),
            [3, 131, 1, 0]
        );
    }

    #[test]
    fn dpi_shift_encodes_dpi_over_fifty_little_endian() {
        // 1600/50 = 32
        assert_eq!(
            Binding::Dpi(DpiAction::Shift(1600)).encode(),
            [102, 5, 32, 0]
        );
        // 26000/50 = 520 = 0x0208, so it spans both bytes.
        assert_eq!(
            Binding::Dpi(DpiAction::Shift(26000)).encode(),
            [102, 5, 0x08, 0x02]
        );
    }

    #[test]
    fn keystroke_packs_modifier_mask_and_code() {
        let b: Binding = "key:ctrl+shift+c".parse().unwrap();
        assert_eq!(b.encode(), [2, 0b0011, 6, 0]);
        let plain: Binding = "key:f5".parse().unwrap();
        assert_eq!(plain.encode(), [2, 0, 62, 0]);
    }

    #[test]
    fn modifier_mask_bit_order() {
        let all = Modifiers {
            ctrl: true,
            shift: true,
            alt: true,
            meta: true,
        };
        assert_eq!(all.mask(), 0b1111);
        assert_eq!(
            Modifiers {
                meta: true,
                ..Default::default()
            }
            .mask(),
            0b1000
        );
    }

    #[test]
    fn binding_parsing_round_trips_and_rejects_junk() {
        assert_eq!("disabled".parse::<Binding>().unwrap(), Binding::Disabled);
        assert_eq!(
            "mute".parse::<Binding>().unwrap(),
            Binding::Media(MediaAction::Mute)
        );
        assert_eq!(
            "macro:3:toggle".parse::<Binding>().unwrap(),
            Binding::Macro {
                id: 3,
                mode: MacroMode::Toggle
            }
        );
        assert!("key:notakey".parse::<Binding>().is_err());
        assert!(
            "dpi-shift:1234".parse::<Binding>().is_err(),
            "not a multiple of 50"
        );
        assert!("nonsense".parse::<Binding>().is_err());
    }

    #[test]
    fn button_names_and_aliases() {
        assert_eq!("left".parse::<Button>().unwrap(), Button::Left);
        assert_eq!("mouse4".parse::<Button>().unwrap(), Button::Back);
        assert_eq!("DPI".parse::<Button>().unwrap(), Button::DpiCycle);
        assert!("thumb7".parse::<Button>().is_err());
    }

    #[test]
    fn validate_refuses_to_orphan_left_click() {
        let mut buttons = Buttons::default();
        assert!(buttons.validate().is_ok());
        buttons.set(Button::Left, Binding::Disabled);
        assert!(
            buttons.validate().is_err(),
            "a mouse with no left-click needs a hardware reset to recover"
        );
        // Rebinding left-click elsewhere is fine.
        buttons.set(Button::Middle, Binding::Mouse(MouseAction::LeftClick));
        assert!(buttons.validate().is_ok());
    }

    #[test]
    fn macro_pages_three_events_each() {
        let m = Macro {
            events: (0..4)
                .map(|i| MacroEvent {
                    code: 4 + i as u8,
                    press: i % 2 == 0,
                    delay_ms: 10,
                })
                .collect(),
        };
        let frags = m.fragments(0);
        assert_eq!(frags.len(), 2, "4 events span 2 pages");
        assert_eq!(&frags[0][0..7], &[0x03, 0x05, 0xfb, 0, 0, 2, 4]);
        assert_eq!(&frags[1][0..7], &[0x03, 0x05, 0xfb, 0, 1, 2, 4]);
        // Event 0: press, delay 10 -> [0x80, 10, code]
        assert_eq!(&frags[0][7..10], &[0x80, 10, 4]);
        // Event 1: release -> high bit clear
        assert_eq!(&frags[0][10..13], &[0x00, 10, 5]);
    }

    #[test]
    fn macro_delay_spans_both_bytes_and_never_zero() {
        let m = Macro {
            events: vec![
                MacroEvent {
                    code: 4,
                    press: true,
                    delay_ms: 300,
                },
                MacroEvent {
                    code: 4,
                    press: false,
                    delay_ms: 0,
                },
            ],
        };
        let frags = m.fragments(1);
        // 300 = 0x012c, press bit set -> 0x80 | 0x01
        assert_eq!(&frags[0][7..10], &[0x81, 0x2c, 4]);
        // A zero delay is floored to 1, matching CORE.
        assert_eq!(&frags[0][10..13], &[0x00, 1, 4]);
    }

    #[test]
    fn key_table_matches_hid_usage_ids() {
        assert_eq!(key_code("a"), Some(4));
        assert_eq!(key_code("Z"), Some(29));
        assert_eq!(key_code("1"), Some(30));
        assert_eq!(key_code("0"), Some(39));
        assert_eq!(key_code("enter"), Some(40));
        assert_eq!(key_code("tab"), Some(43));
        assert_eq!(key_name(62), Some("f5"));
        assert_eq!(key_code("nope"), None);
    }
}
