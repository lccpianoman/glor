//! Interactive terminal UI over the same state cache the CLI uses.

use anyhow::Result;
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{
    self, disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{execute, queue};
use std::io::{self, Write};
use std::time::Duration;

use crate::device;
use crate::protocol::{
    Effect, Rgb, DEBOUNCE_MAX_MS, DEFAULT_PALETTE, MAX_STAGES, MIN_STAGES, POLLING_RATES,
    SPEED_RAW_MAX,
};
use crate::state::State;

/// A row in the menu. `Setting` rows are adjusted in place with left/right.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Row {
    Heading(&'static str),
    Profile,
    Effect,
    Palette,
    Speed,
    Brightness,
    Polling,
    Debounce,
    Lod,
    MotionSync,
    ActiveStage,
    StageDpi(usize),
    StageCount,
    Apply,
    Reset,
    Quit,
}

impl Row {
    fn selectable(self) -> bool {
        !matches!(self, Row::Heading(_))
    }
}

/// Palette presets cycled through on the Palette row.
const PALETTES: [(&str, [Rgb; 7]); 5] = [
    ("rainbow", DEFAULT_PALETTE),
    ("red", [Rgb::new(255, 0, 0); 7]),
    ("green", [Rgb::new(0, 255, 0); 7]),
    ("blue", [Rgb::new(0, 80, 255); 7]),
    ("white", [Rgb::new(255, 255, 255); 7]),
];

struct App {
    state: State,
    rows: Vec<Row>,
    selected: usize,
    status: String,
    device: String,
    battery: String,
    palette_idx: usize,
    dirty_lighting: bool,
    dirty_settings: bool,
    dirty_profile: bool,
    dirty_buttons: bool,
    quit_armed: bool,
    battery_listener: Option<device::BatteryListener>,
}

impl App {
    fn new() -> Result<App> {
        let state = State::load()?;
        let device = match device::find() {
            Ok(f) => format!(
                "{} ({})",
                f.name,
                if f.wireless { "wireless" } else { "wired" }
            ),
            Err(_) => "not connected".to_string(),
        };
        let battery = match &state.battery {
            Some(b) => format!(
                "{}%{} ({})",
                b.percent,
                if b.charging { " chg" } else { "" },
                b.age()
            ),
            None => "unknown".to_string(),
        };
        let mut app = App {
            state,
            rows: Vec::new(),
            selected: 0,
            status: "Ready".to_string(),
            device,
            battery,
            palette_idx: 0,
            dirty_lighting: false,
            dirty_settings: false,
            dirty_profile: false,
            dirty_buttons: false,
            quit_armed: false,
            battery_listener: device::BatteryListener::open().ok(),
        };
        app.rebuild_rows();
        app.selected = app.rows.iter().position(|r| r.selectable()).unwrap_or(0);
        Ok(app)
    }

    /// The stage rows depend on `stage_count`, so the row list is rebuilt whenever it changes.
    fn rebuild_rows(&mut self) {
        let mut rows = vec![
            Row::Heading("Profile"),
            Row::Profile,
            Row::Heading("Lighting"),
            Row::Effect,
            Row::Palette,
            Row::Speed,
            Row::Brightness,
            Row::Heading("Performance"),
            Row::Polling,
            Row::Debounce,
            Row::Lod,
            Row::MotionSync,
            Row::Heading("DPI"),
            Row::StageCount,
            Row::ActiveStage,
        ];
        for i in 0..self.state.active().settings.stage_count as usize {
            rows.push(Row::StageDpi(i));
        }
        rows.push(Row::Heading(""));
        rows.push(Row::Apply);
        rows.push(Row::Reset);
        rows.push(Row::Quit);
        self.rows = rows;
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len() - 1;
        }
    }

    fn label(&self, row: Row) -> String {
        let l = &self.state.active().lighting;
        let s = &self.state.active().settings;
        match row {
            Row::Heading(h) => h.to_string(),
            Row::Profile => format!(
                "Active profile  < {} of {} >",
                self.state.active_profile + 1,
                crate::protocol::PROFILE_COUNT
            ),
            Row::Effect => format!("Effect          < {} >", l.effect.label()),
            Row::Palette => format!("Colours         < {} >", PALETTES[self.palette_idx].0),
            Row::Speed => {
                if l.effect.is_animated() {
                    format!("Speed           < {}/20 >", l.speed)
                } else {
                    format!("Speed             {}/20 (effect is static)", l.speed)
                }
            }
            Row::Brightness => format!(
                "Brightness      < {}/20 >  (wired {})",
                l.brightness_wireless, l.brightness_wired
            ),
            Row::Polling => format!("Polling rate    < {} Hz >", s.polling_hz()),
            Row::Debounce => format!("Debounce        < {} ms >", s.debounce_ms),
            Row::Lod => format!("Lift-off        < {} mm >", s.lod_mm),
            Row::MotionSync => format!(
                "Motion sync     < {} >",
                if s.motion_sync { "on" } else { "off" }
            ),
            Row::StageCount => format!("Stage count     < {} >", s.stage_count),
            Row::ActiveStage => format!("Active stage    < {} >", s.active_stage + 1),
            Row::StageDpi(i) => {
                let marker = if i == s.active_stage as usize {
                    "*"
                } else {
                    " "
                };
                format!(
                    "  {marker} stage {}     < {:>5} DPI >",
                    i + 1,
                    s.stage_dpis[i]
                )
            }
            Row::Apply => {
                if self.has_pending() {
                    format!("Apply changes  [{}]", self.pending_summary())
                } else {
                    "Apply changes  (nothing pending)".to_string()
                }
            }
            Row::Reset => "Reset to factory defaults".to_string(),
            Row::Quit => "Quit".to_string(),
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.rows.len() as isize;
        let mut idx = self.selected as isize;
        for _ in 0..len {
            idx = (idx + delta).rem_euclid(len);
            if self.rows[idx as usize].selectable() {
                self.selected = idx as usize;
                return;
            }
        }
    }

    /// Steps the selected setting. `delta` is -1 or +1.
    fn adjust(&mut self, delta: i32) {
        let row = self.rows[self.selected];
        match row {
            Row::Profile => {
                let count = crate::protocol::PROFILE_COUNT as i32;
                let cur = self.state.active_profile as i32;
                self.state.active_profile = (cur + delta).rem_euclid(count) as u8;
                // Switching profiles re-pushes that profile's configuration, so the mouse
                // and the cache cannot drift apart.
                self.dirty_profile = true;
                self.dirty_lighting = true;
                self.dirty_settings = true;
                self.rebuild_rows();
            }
            Row::Effect => {
                let cur = Effect::ALL
                    .iter()
                    .position(|e| *e == self.state.active().lighting.effect)
                    .unwrap_or(0);
                let next = wrap(cur, delta, Effect::ALL.len());
                self.state.active_mut().lighting.effect = Effect::ALL[next];
                self.dirty_lighting = true;
            }
            Row::Palette => {
                self.palette_idx = wrap(self.palette_idx, delta, PALETTES.len());
                self.state.active_mut().lighting.colors = PALETTES[self.palette_idx].1;
                self.state.active_mut().lighting.color_count = None;
                self.dirty_lighting = true;
            }
            Row::Speed => {
                // 1..=20: the firmware's real range. CORE never sends 0, which would stall
                // the animation.
                let cur = self.state.active_mut().lighting.speed as i32;
                self.state.active_mut().lighting.speed =
                    (cur + delta).clamp(1, SPEED_RAW_MAX as i32) as u8;
                self.dirty_lighting = true;
            }
            Row::Brightness => {
                // 0..=20; 0 is legal here and turns the LEDs off. Both fields move together;
                // the CLI can set them independently.
                let cur = self.state.active_mut().lighting.brightness_wireless as i32;
                let next = (cur + delta).clamp(0, SPEED_RAW_MAX as i32) as u8;
                self.state.active_mut().lighting.brightness_wireless = next;
                self.state.active_mut().lighting.brightness_wired = next;
                self.dirty_lighting = true;
            }
            Row::MotionSync => {
                self.state.active_mut().settings.motion_sync =
                    !self.state.active_mut().settings.motion_sync;
                self.dirty_settings = true;
            }
            Row::Polling => {
                let rates: Vec<u16> = POLLING_RATES.iter().map(|(_, hz)| *hz).collect();
                let cur = rates
                    .iter()
                    .position(|hz| *hz == self.state.active_mut().settings.polling_hz())
                    .unwrap_or(3);
                let next = wrap(cur, delta, rates.len());
                self.state.active_mut().settings.polling_code = POLLING_RATES
                    .iter()
                    .find(|(_, hz)| *hz == rates[next])
                    .unwrap()
                    .0;
                self.dirty_settings = true;
            }
            Row::Debounce => {
                let cur = self.state.active_mut().settings.debounce_ms as i32;
                self.state.active_mut().settings.debounce_ms =
                    (cur + delta * 2).clamp(0, DEBOUNCE_MAX_MS as i32) as u8;
                self.dirty_settings = true;
            }
            Row::Lod => {
                self.state.active_mut().settings.lod_mm =
                    if self.state.active_mut().settings.lod_mm == 1 {
                        2
                    } else {
                        1
                    };
                self.dirty_settings = true;
            }
            Row::StageCount => {
                let cur = self.state.active_mut().settings.stage_count as i32;
                let next = (cur + delta).clamp(MIN_STAGES as i32, MAX_STAGES as i32) as u8;
                if next != self.state.active_mut().settings.stage_count {
                    self.state.active_mut().settings.stage_count = next;
                    // Give a newly enabled stage a sane DPI instead of an invalid 0.
                    for i in 0..next as usize {
                        if self.state.active_mut().settings.stage_dpis[i] == 0 {
                            self.state.active_mut().settings.stage_dpis[i] = 800;
                        }
                    }
                    if self.state.active_mut().settings.active_stage >= next {
                        self.state.active_mut().settings.active_stage = next - 1;
                    }
                    self.rebuild_rows();
                    self.dirty_settings = true;
                }
            }
            Row::ActiveStage => {
                let count = self.state.active_mut().settings.stage_count as i32;
                let cur = self.state.active_mut().settings.active_stage as i32;
                self.state.active_mut().settings.active_stage =
                    (cur + delta).rem_euclid(count) as u8;
                self.dirty_settings = true;
            }
            Row::StageDpi(i) => {
                let cur = self.state.active_mut().settings.stage_dpis[i] as i32;
                self.state.active_mut().settings.stage_dpis[i] =
                    (cur + delta * 50).clamp(100, 26_000) as u16;
                self.dirty_settings = true;
            }
            _ => {}
        }
    }

    /// Whether anything is waiting to be applied.
    fn has_pending(&self) -> bool {
        self.dirty_profile || self.dirty_lighting || self.dirty_settings || self.dirty_buttons
    }

    fn pending_summary(&self) -> String {
        let mut parts = Vec::new();
        if self.dirty_profile {
            parts.push("profile");
        }
        if self.dirty_lighting {
            parts.push("lighting");
        }
        if self.dirty_settings {
            parts.push("settings");
        }
        if self.dirty_buttons {
            parts.push("buttons");
        }
        parts.join(", ")
    }

    /// Writes the pending payloads to the mouse and persists the cache.
    ///
    /// Nothing reaches the device until this runs — editing a row only changes the working
    /// copy. The cache is saved only after a successful write, so it never claims a
    /// configuration the mouse does not have.
    fn apply(&mut self) -> Result<()> {
        self.state.normalize();
        let profile = self.state.active_profile;
        if self.dirty_profile {
            device::switch_profile(profile, false)?;
            self.dirty_profile = false;
        }
        if self.dirty_lighting {
            let frags = self.state.active().lighting.fragments(profile);
            let refs: Vec<&[u8]> = frags.iter().map(|f| f.as_slice()).collect();
            device::apply("lighting", &refs, false)?;
            self.dirty_lighting = false;
        }
        if self.dirty_settings {
            let frags = self.state.active().settings.fragments(profile);
            let refs: Vec<&[u8]> = frags.iter().map(|f| f.as_slice()).collect();
            device::apply("settings", &refs, false)?;
            self.dirty_settings = false;
        }
        if self.dirty_buttons {
            let frags = self.state.active().buttons.fragments(profile);
            let refs: Vec<&[u8]> = frags.iter().map(|f| f.as_slice()).collect();
            device::apply("buttons", &refs, false)?;
            self.dirty_buttons = false;
        }
        self.state.save()?;
        Ok(())
    }

    /// Persists a freshly observed battery reading.
    ///
    /// This is cache, not configuration, so it saves immediately rather than waiting for
    /// Apply — it describes the mouse rather than changing it.
    fn record_battery(&mut self, battery: crate::protocol::Battery) {
        let reading = crate::state::BatteryReading::new(battery);
        self.battery = format!(
            "{}%{}",
            reading.percent,
            if reading.charging { " chg" } else { "" }
        );
        self.state.battery = Some(reading);
        let _ = self.state.save();
    }

    /// Returns true when the app should exit.
    fn activate(&mut self) -> Result<bool> {
        match self.rows[self.selected] {
            Row::Quit => return Ok(true),
            Row::Apply => {
                // Apply is the only path that touches the mouse.
                if !self.has_pending() {
                    self.status = "Nothing to apply".to_string();
                    return Ok(false);
                }
                let summary = self.pending_summary();
                match self.apply() {
                    Ok(()) => self.status = format!("Applied: {summary}"),
                    Err(e) => self.status = format!("Error: {e:#}"),
                }
            }
            Row::Reset => {
                // Staged like any other edit; nothing is written until Apply.
                let battery = self.state.battery;
                self.state = State::default();
                self.state.battery = battery;
                self.palette_idx = 0;
                self.rebuild_rows();
                self.dirty_profile = true;
                self.dirty_lighting = true;
                self.dirty_settings = true;
                self.dirty_buttons = true;
                self.status = "Factory defaults staged — press Apply".to_string();
            }
            _ => {
                self.status = "Use ←→ to change a value, then Apply".to_string();
            }
        }
        Ok(false)
    }
}

fn wrap(current: usize, delta: i32, len: usize) -> usize {
    ((current as i32 + delta).rem_euclid(len as i32)) as usize
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn draw_line(
    out: &mut io::Stdout,
    x: u16,
    y: u16,
    w: u16,
    text: &str,
    highlight: bool,
) -> Result<()> {
    let clipped = truncate(text, w as usize);
    let padded = format!(
        "{clipped}{}",
        " ".repeat(w as usize - clipped.chars().count())
    );
    queue!(out, MoveTo(x, y), Print("│"))?;
    if highlight {
        queue!(
            out,
            SetAttribute(Attribute::Reverse),
            Print(&padded),
            SetAttribute(Attribute::NoReverse)
        )?;
    } else {
        queue!(out, Print(&padded))?;
    }
    queue!(out, Print("│"))?;
    Ok(())
}

fn draw(out: &mut io::Stdout, app: &App) -> Result<()> {
    let (w, h) = terminal::size()?;
    let title = format!("glor — {}   ·   battery {}", app.device, app.battery);
    let help = "↑↓/jk move   ←→/hl change   Enter select   r reload   q quit";

    let labels: Vec<String> = app
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| match row {
            Row::Heading("") => String::new(),
            Row::Heading(hh) => format!("── {hh} "),
            _ => {
                let prefix = if i == app.selected { "▸ " } else { "  " };
                format!("{prefix}{}", app.label(*row))
            }
        })
        .collect();

    let status = format!("Status: {}", app.status);
    let inner_w = labels
        .iter()
        .map(|l| l.chars().count())
        .chain([
            title.chars().count(),
            help.chars().count(),
            status.chars().count(),
        ])
        .max()
        .unwrap_or(48)
        .max(48)
        + 2;
    let panel_w = inner_w as u16 + 2;
    let panel_h = labels.len() as u16 + 6;

    if w < panel_w || h < panel_h {
        let msg = format!("Terminal too small: need {panel_w}x{panel_h}, have {w}x{h}");
        let msg = truncate(&msg, w.saturating_sub(1) as usize);
        queue!(
            out,
            MoveTo(0, 0),
            Clear(ClearType::All),
            MoveTo(0, h / 2),
            Print(msg)
        )?;
        out.flush()?;
        return Ok(());
    }

    let x0 = (w - panel_w) / 2;
    let y0 = (h - panel_h) / 2;
    let iw = inner_w as u16;

    queue!(out, MoveTo(0, 0), Clear(ClearType::All))?;
    queue!(
        out,
        MoveTo(x0, y0),
        Print(format!("┌{}┐", "─".repeat(iw as usize)))
    )?;

    let mut row_y = y0 + 1;
    draw_line(out, x0, row_y, iw, &format!(" {title}"), false)?;
    row_y += 1;
    draw_line(out, x0, row_y, iw, &format!(" {help}"), false)?;
    row_y += 1;
    draw_line(out, x0, row_y, iw, "", false)?;
    row_y += 1;

    for (i, label) in labels.iter().enumerate() {
        let selected = i == app.selected && app.rows[i].selectable();
        draw_line(out, x0, row_y, iw, &format!(" {label}"), selected)?;
        row_y += 1;
    }

    draw_line(out, x0, row_y, iw, "", false)?;
    row_y += 1;
    draw_line(out, x0, row_y, iw, &format!(" {status}"), false)?;
    row_y += 1;
    queue!(
        out,
        MoveTo(x0, row_y),
        Print(format!("└{}┘", "─".repeat(iw as usize)))
    )?;
    out.flush()?;
    Ok(())
}

pub fn run() -> Result<()> {
    let mut out = io::stdout();
    let mut app = App::new()?;

    execute!(out, EnterAlternateScreen, Hide)?;
    enable_raw_mode()?;

    let result = (|| -> Result<()> {
        draw(&mut out, &app)?;
        loop {
            // A short poll doubles as the battery tick: the mouse broadcasts roughly once a
            // minute, so checking a few times a second costs nothing and shows it promptly.
            if !event::poll(Duration::from_millis(200))? {
                if let Some(battery) = app.battery_listener.as_ref().and_then(|l| l.poll()) {
                    app.record_battery(battery);
                    draw(&mut out, &app)?;
                }
                continue;
            }
            match event::read()? {
                Event::Resize(_, _) => {}
                Event::Key(key) => {
                    let ctrl_c = key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL);
                    let quitting =
                        ctrl_c || key.code == KeyCode::Char('q') || key.code == KeyCode::Esc;
                    if quitting {
                        // Leaving with staged edits would silently discard them, so ask once.
                        if app.has_pending() && !app.quit_armed && !ctrl_c {
                            app.quit_armed = true;
                            app.status = format!(
                                "Unapplied changes ({}) — q again to discard",
                                app.pending_summary()
                            );
                            draw(&mut out, &app)?;
                            continue;
                        }
                        break;
                    }
                    // Any other key means they are still working; disarm the quit prompt.
                    app.quit_armed = false;

                    match key.code {
                        KeyCode::Up | KeyCode::Char('k') => app.move_selection(-1),
                        KeyCode::Down | KeyCode::Char('j') => app.move_selection(1),
                        KeyCode::Left | KeyCode::Char('h') => {
                            app.adjust(-1);
                            app.status = "Staged — press Apply".to_string();
                        }
                        KeyCode::Right | KeyCode::Char('l') => {
                            app.adjust(1);
                            app.status = "Staged — press Apply".to_string();
                        }
                        KeyCode::Char('r') => match State::load() {
                            Ok(s) => {
                                app.state = s;
                                app.dirty_profile = false;
                                app.dirty_lighting = false;
                                app.dirty_settings = false;
                                app.dirty_buttons = false;
                                app.rebuild_rows();
                                app.status = "Reloaded from disk, staged edits discarded".into();
                            }
                            Err(e) => app.status = format!("Error: {e:#}"),
                        },
                        KeyCode::Enter => {
                            // Applying paces its writes to match the firmware, so it takes
                            // a second or two — say so before blocking.
                            if app.rows[app.selected] == Row::Apply && app.has_pending() {
                                app.status = "Applying…".to_string();
                                draw(&mut out, &app)?;
                            }
                            if app.activate()? {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
            draw(&mut out, &app)?;
        }
        Ok(())
    })();

    disable_raw_mode()?;
    execute!(out, Show, LeaveAlternateScreen)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An App with no device attached, for logic that does not touch hardware.
    fn test_app(rows: Vec<Row>) -> App {
        App {
            state: State::default(),
            rows,
            selected: 1,
            status: String::new(),
            device: String::new(),
            battery: String::new(),
            palette_idx: 0,
            dirty_lighting: false,
            dirty_settings: false,
            dirty_profile: false,
            dirty_buttons: false,
            quit_armed: false,
            battery_listener: None,
        }
    }

    #[test]
    fn editing_stages_changes_without_touching_the_device() {
        // The whole point of deferred apply: adjusting a row must only mark state dirty.
        let mut app = test_app(vec![Row::Heading("x"), Row::Speed]);
        assert!(!app.has_pending());
        app.adjust(1);
        assert!(app.has_pending(), "an edit must be staged");
        assert_eq!(app.pending_summary(), "lighting");
        // Settings rows stage separately from lighting ones.
        app.rows = vec![Row::Heading("x"), Row::Debounce];
        app.adjust(1);
        assert_eq!(app.pending_summary(), "lighting, settings");
    }

    #[test]
    fn a_profile_switch_stages_everything_for_that_profile() {
        let mut app = test_app(vec![Row::Heading("x"), Row::Profile]);
        app.adjust(1);
        assert_eq!(app.state.active_profile, 1);
        // The device holds one profile's config at a time, so all of it must be re-pushed.
        assert!(app.dirty_profile && app.dirty_lighting && app.dirty_settings);
    }

    #[test]
    fn headings_are_skipped_when_moving() {
        let mut app = test_app(vec![
            Row::Heading("x"),
            Row::Effect,
            Row::Heading("y"),
            Row::Speed,
        ]);
        app.move_selection(1);
        assert_eq!(app.selected, 3, "skips the heading at index 2");
        app.move_selection(1);
        assert_eq!(app.selected, 1, "wraps past headings");
    }
}
