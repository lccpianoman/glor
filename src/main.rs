mod buttons;
mod device;
mod protocol;
mod state;
mod tui;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand};
use std::io::Write;
use std::time::Duration;

use buttons::{Binding, Button, Buttons, Macro, MacroEvent};
use protocol::{
    brightness_level_from_percent, polling_code_for, sanitize_debounce, speed_level_from_percent,
    validate_dpi, Effect, Lighting, Rgb, Settings, DEFAULT_PALETTE, MAX_STAGES, MIN_STAGES,
    SPEED_RAW_MAX,
};
use state::{Profile, State};

#[derive(Parser)]
#[command(
    name = "glor",
    version,
    about = "CLI/TUI for Pixart-based Glorious mice (Model O 2 / I 2 family)",
    long_about = "Configures Pixart-based Glorious mice on Linux.\n\n\
                  The firmware is write-only: it accepts configuration but never reports it \
                  back. glor therefore mirrors your configuration in ~/.config/glor/state.json \
                  and rewrites the full payload on every change, so adjusting one setting does \
                  not reset the others."
)]
struct Cli {
    /// Hexdump every outgoing fragment to stderr before sending.
    #[arg(long, global = true)]
    trace: bool,

    /// Edit a specific onboard profile (1-3) instead of the active one.
    ///
    /// The change is written to that profile on the device without switching to it.
    #[arg(long, global = true, value_parser = clap::value_parser!(u8).range(1..=3))]
    profile: Option<u8>,

    /// Never offer to re-run under sudo when the device is not writable.
    #[arg(long, global = true)]
    no_sudo: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show the connected mouse.
    Detect,
    /// Diagnose interface selection and hidraw permissions.
    Doctor {
        /// Print the udev rule instead, for piping into /etc/udev/rules.d/.
        #[arg(long)]
        udev_rule: bool,
    },
    /// Show the cached configuration.
    Info,
    /// Read the battery level.
    ///
    /// The mouse broadcasts battery about every 65 seconds; it cannot be polled, so this
    /// waits for the next broadcast. Plugging or unplugging the cable forces one immediately.
    Battery {
        /// Seconds to wait for a report. Must exceed the ~65s broadcast interval.
        #[arg(long, default_value_t = default_battery_wait())]
        wait: u64,
        /// Keep listening and print every reading as it arrives. Use this as a daemon to
        /// keep the cache warm so plain `glor battery` answers instantly.
        #[arg(long)]
        watch: bool,
        /// Ignore the cache and wait for a fresh broadcast.
        #[arg(long)]
        force: bool,
    },
    /// Launch the interactive TUI.
    Tui,
    /// Set the lighting effect and its parameters.
    Rgb(RgbArgs),
    /// Set effect brightness (0-100). Applies to both wired and wireless unless narrowed.
    Brightness {
        percent: u8,
        /// Only change the wired brightness.
        #[arg(long, conflicts_with = "wireless_only")]
        wired_only: bool,
        /// Only change the wireless brightness.
        #[arg(long)]
        wireless_only: bool,
    },
    /// Set animation speed (0-100).
    Speed {
        /// Speed as a percentage. Omit when using --raw.
        #[arg(required_unless_present = "raw")]
        percent: Option<u8>,
        /// Send a raw 0-20 speed byte instead of a percentage.
        #[arg(long, conflicts_with = "percent")]
        raw: Option<u8>,
    },
    /// Set click debounce in milliseconds (even, 0-16).
    Debounce { ms: u8 },
    /// Set the DPI stages, e.g. `glor dpi 400,800,1600,3200`.
    Dpi {
        /// Comma-separated DPI values (4-6 stages, multiples of 50, 100-26000).
        stages: String,
        /// Comma-separated per-stage colours, e.g. `ff0000,0000ff,00ff00,ffff00`.
        #[arg(long)]
        colors: Option<String>,
    },
    /// Select the active DPI stage (1-based).
    Stage { index: u8 },
    /// Set the polling rate in Hz (125, 250, 500, 1000).
    Polling { hz: u16 },
    /// Set lift-off distance in mm (1 or 2).
    Lod { mm: u8 },
    /// Enable or disable motion sync.
    MotionSync {
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
    /// Show the current button mapping.
    Buttons,
    /// Bind a button.
    ///
    /// Bindings: disabled, left-click, right-click, middle-click, back, forward,
    /// scroll-up, scroll-down, profile-up, profile-down, dpi-stage-up, dpi-stage-down,
    /// dpi-cycle-up, dpi-cycle-down, dpi-shift:<dpi>, play-pause, stop, mute, volume-up,
    /// volume-down, next-track, previous-track, media-player, battery-check, layer-shift,
    /// key:<ctrl+shift+alt+meta+><key>, macro:<id>[:once|hold|toggle]
    Bind {
        /// Button: left, right, middle, back, forward, dpi.
        button: String,
        /// What to bind it to.
        binding: String,
        /// Permit a mapping with no left-click. Recovering needs a hardware factory reset.
        #[arg(long)]
        allow_no_left_click: bool,
    },
    /// Record a macro into a slot, or clear one.
    ///
    /// `keys` is a comma-separated sequence of key names; each becomes a press followed by
    /// a release. Bind it with `glor bind <button> macro:<id>[:toggle]`.
    Macro {
        /// Macro slot id.
        id: u8,
        /// Comma-separated keys, e.g. `ctrl,c`. Omit to clear the slot.
        keys: Option<String>,
        /// Milliseconds between events.
        #[arg(long, default_value_t = 10)]
        delay: u16,
    },
    /// Show profiles, or switch the active one.
    Profile {
        /// Profile to activate (1-3). Omit to list them.
        #[arg(value_parser = clap::value_parser!(u8).range(1..=3))]
        index: Option<u8>,
    },
    /// Re-send the cached configuration to the mouse.
    Sync,
    /// Reset the cache to factory defaults and push them.
    Reset,
}

/// Default wait for `glor battery`: one broadcast interval plus a little slack, so a single
/// invocation is guaranteed to span at least one heartbeat.
fn default_battery_wait() -> u64 {
    device::BATTERY_HEARTBEAT.as_secs() + 4
}

#[derive(Args)]
struct RgbArgs {
    /// Effect name: off, glorious, seamless-breathing, breathing, solid,
    /// breathing-single, tail, rave, wave.
    effect: String,
    /// Comma-separated colours (1-7), e.g. `ff0000,00ff00`. Defaults to the rainbow.
    #[arg(long)]
    colors: Option<String>,
    /// Animation speed 0-100.
    #[arg(long)]
    speed: Option<u8>,
    /// Brightness 0-100.
    #[arg(long)]
    brightness: Option<u8>,
}

fn parse_colors(spec: &str) -> Result<Vec<Rgb>> {
    let colors: Result<Vec<Rgb>> = spec.split(',').map(|c| c.trim().parse()).collect();
    let colors = colors?;
    if colors.is_empty() || colors.len() > 7 {
        bail!("expected 1-7 colours, got {}", colors.len());
    }
    Ok(colors)
}

fn apply_lighting(lighting: &Lighting, profile: u8, trace: bool) -> Result<()> {
    let frags = lighting.fragments(profile);
    let refs: Vec<&[u8]> = frags.iter().map(|f| f.as_slice()).collect();
    device::apply("lighting", &refs, trace)
}

fn apply_settings(settings: &Settings, profile: u8, trace: bool) -> Result<()> {
    let frags = settings.fragments(profile);
    let refs: Vec<&[u8]> = frags.iter().map(|f| f.as_slice()).collect();
    device::apply("settings", &refs, trace)
}

fn apply_buttons(buttons: &Buttons, profile: u8, trace: bool) -> Result<()> {
    let frags = buttons.fragments(profile);
    let refs: Vec<&[u8]> = frags.iter().map(|f| f.as_slice()).collect();
    device::apply("buttons", &refs, trace)
}

fn update_buttons(
    target: Option<u8>,
    trace: bool,
    f: impl FnOnce(&mut Profile) -> Result<()>,
) -> Result<Profile> {
    let mut state = State::load()?;
    let (profile, idx) = state.profile_mut(target)?;
    f(profile)?;
    state.normalize();
    let edited = state.profiles[idx as usize].clone();
    apply_buttons(&edited.buttons, idx, trace)?;
    state.save()?;
    Ok(edited)
}

/// Load state, mutate one profile, push the affected payload, then persist.
///
/// `target` selects which profile to edit; `None` means the active one. Persisting only
/// after a successful write keeps the cache honest: if the mouse rejects the payload, the
/// file still reflects what is actually on the device.
/// Returns the edited profile so callers can report what they just set — which is not
/// necessarily the active profile, when `--profile` was used.
fn update_lighting(
    target: Option<u8>,
    trace: bool,
    f: impl FnOnce(&mut Profile) -> Result<()>,
) -> Result<Profile> {
    let mut state = State::load()?;
    let (profile, idx) = state.profile_mut(target)?;
    f(profile)?;
    state.normalize();
    let edited = state.profiles[idx as usize].clone();
    apply_lighting(&edited.lighting, idx, trace)?;
    state.save()?;
    Ok(edited)
}

fn update_settings(
    target: Option<u8>,
    trace: bool,
    f: impl FnOnce(&mut Profile) -> Result<()>,
) -> Result<Profile> {
    let mut state = State::load()?;
    let (profile, idx) = state.profile_mut(target)?;
    f(profile)?;
    state.normalize();
    let edited = state.profiles[idx as usize].clone();
    apply_settings(&edited.settings, idx, trace)?;
    state.save()?;
    Ok(edited)
}

fn print_info(state: &State) -> Result<()> {
    let device_line = match device::find() {
        Ok(found) => format!(
            "{} ({:04x}:{:04x}, {})",
            found.name,
            device::VENDOR_ID,
            found.product_id,
            if found.wireless { "wireless" } else { "wired" }
        ),
        Err(_) => "not connected".to_string(),
    };

    let l = &state.active().lighting;
    let s = &state.active().settings;

    println!("Device:    {device_line}");
    println!("State:     {}", state::state_path()?.display());
    println!(
        "Profile:   {} of {}",
        state.active_profile + 1,
        protocol::PROFILE_COUNT
    );
    println!();
    println!("Lighting");
    println!("  effect       {} ({})", l.effect.name(), l.effect.label());
    if l.effect.is_animated() {
        println!("  speed        0x{:02x} ({}/20)", l.speed, l.speed);
    }
    println!(
        "  brightness   {}/20 wireless, {}/20 wired",
        l.brightness_wireless, l.brightness_wired
    );
    if l.effect.uses_palette() {
        let shown = l.effective_color_count() as usize;
        let palette: Vec<String> = l.colors[..shown.min(7)]
            .iter()
            .map(|c| c.to_string())
            .collect();
        println!("  colours      {}", palette.join(" "));
    }
    println!();
    println!("Settings");
    println!("  polling      {} Hz", s.polling_hz());
    println!("  debounce     {} ms", s.debounce_ms);
    println!("  lift-off     {} mm", s.lod_mm);
    println!(
        "  motion sync  {}",
        if s.motion_sync { "on" } else { "off" }
    );
    println!(
        "  active DPI   {} (stage {} of {})",
        s.active_dpi(),
        s.active_stage + 1,
        s.stage_count
    );
    println!("  stages");
    for i in 0..s.stage_count as usize {
        let marker = if i == s.active_stage as usize {
            "*"
        } else {
            " "
        };
        println!(
            "   {marker} {}: {:>5} DPI  {}",
            i + 1,
            s.stage_dpis[i],
            s.stage_colors[i]
        );
    }
    println!();
    match &state.battery {
        Some(b) => println!(
            "Battery:   {}%{} (last seen {})",
            b.percent,
            if b.charging { ", charging" } else { "" },
            b.age()
        ),
        None => println!("Battery:   no reading yet — run `glor battery`"),
    }
    Ok(())
}

fn print_battery(reading: &state::BatteryReading, fresh: bool) {
    let charging = if reading.charging { ", charging" } else { "" };
    if fresh {
        println!("Battery: {}%{charging}", reading.percent);
    } else {
        println!(
            "Battery: {}%{charging} (as of {})",
            reading.percent,
            reading.age()
        );
    }
}

/// Reports battery, preferring a cached reading that is still current.
///
/// There is no way to ask the mouse for its battery level; it broadcasts one about every
/// [`device::BATTERY_HEARTBEAT`]. So a cached reading younger than one interval is as
/// current as anything a fresh listen could produce, and returns instantly. Only when the
/// cache is older than that does this block waiting for the next broadcast.
fn battery_command(wait: u64, watch: bool, force: bool) -> Result<()> {
    if watch {
        println!("Watching for battery broadcasts (~every 65s). Ctrl-C to stop.");
        loop {
            if let Some(b) = device::listen_battery(Duration::from_secs(wait.max(1)))? {
                let reading = state::BatteryReading::new(b);
                println!(
                    "  {}%{} ",
                    reading.percent,
                    if reading.charging { ", charging" } else { "" }
                );
                let mut state = State::load()?;
                state.battery = Some(reading);
                state.save()?;
            }
        }
    }

    let cached = State::load()?.battery;
    if !force {
        if let Some(b) = cached {
            // A reading younger than one broadcast interval cannot be improved on by
            // waiting — the mouse has not sent anything newer.
            if b.age_secs() < device::BATTERY_HEARTBEAT.as_secs() {
                print_battery(&b, false);
                return Ok(());
            }
        }
    }
    if let Some(b) = cached {
        print_battery(&b, false);
    }
    println!("Waiting up to {wait}s for the next broadcast (the mouse sends one ~every 65s)...");

    let result = device::listen_battery_with(Duration::from_secs(wait), |elapsed| {
        print!("\r  {}s ", elapsed.as_secs());
        let _ = std::io::stdout().flush();
    })?;
    print!("\r             \r");
    std::io::stdout().flush()?;

    match result {
        Some(b) => {
            let reading = state::BatteryReading::new(b);
            let mut state = State::load()?;
            state.battery = Some(reading);
            state.save()?;
            print_battery(&reading, true);
        }
        None => {
            println!("No battery report in {wait}s.");
            println!(
                "The broadcast interval is about 65s, so allow at least that \
                 (`--wait 70`), or plug/unplug the cable to force one."
            );
        }
    }
    Ok(())
}

/// Restores the default SIGPIPE handler.
///
/// Rust ignores SIGPIPE, so writing to a closed pipe surfaces as an `EPIPE` panic instead
/// of a quiet exit — `glor info | head` would otherwise print a backtrace.
fn restore_sigpipe() {
    // SAFETY: setting a signal disposition to SIG_DFL before any threads are spawned.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// Set on the child so an elevated run can never try to elevate again.
const ELEVATED_MARKER: &str = "GLOR_ELEVATED";

fn is_tty() -> bool {
    // SAFETY: isatty on a fixed descriptor has no preconditions.
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
}

/// Re-runs this command under `sudo` after a permission failure.
///
/// `$GLOR_STATE` is passed explicitly so the elevated process writes the *user's* config
/// rather than root's — sudo resets `HOME`, and without this the two would silently
/// diverge. `state::save_to` then hands ownership back via `SUDO_UID`.
fn rerun_elevated() -> Result<std::process::ExitStatus> {
    let exe = std::env::current_exe().context("locating the glor binary")?;
    let state = state::state_path()?;
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    std::process::Command::new("sudo")
        .arg(format!("{ELEVATED_MARKER}=1"))
        .arg(format!("GLOR_STATE={}", state.display()))
        .arg(exe)
        .args(args)
        .status()
        .context("running sudo")
}

/// Turns a device failure into either useful guidance or an elevated retry.
///
/// Escalation is offered only when the problem is genuinely permissions, only when there is
/// a terminal to type a password into, and never from an already-elevated process.
fn handle_failure(err: anyhow::Error, no_sudo: bool) -> Result<()> {
    if !matches!(
        device::access_problem(),
        Some(device::AccessProblem::PermissionDenied { .. })
    ) {
        return Err(err);
    }

    eprintln!("{}", device::permission_help());

    let already_elevated = std::env::var_os(ELEVATED_MARKER).is_some();
    if no_sudo || already_elevated || !is_tty() {
        eprintln!();
        return Err(err);
    }

    eprintln!("\nRetrying this command with sudo...\n");
    let status = rerun_elevated()?;
    if status.success() {
        Ok(())
    } else {
        Err(err)
    }
}

fn main() -> Result<()> {
    restore_sigpipe();
    let cli = Cli::parse();
    let no_sudo = cli.no_sudo;
    match run(cli) {
        Ok(()) => Ok(()),
        Err(e) => handle_failure(e, no_sudo),
    }
}

fn run(cli: Cli) -> Result<()> {
    let trace = cli.trace;
    // CLI profiles are 1-based; the wire and the state cache are 0-based.
    let profile = cli.profile.map(|p| p - 1);

    match cli.command {
        Commands::Detect => println!("{}", device::detect()?),
        Commands::Doctor { udev_rule } => {
            if udev_rule {
                // Printed verbatim so it can be piped straight to the rules directory.
                print!("{}", device::UDEV_RULES);
            } else {
                println!("{}", device::doctor()?);
            }
        }
        Commands::Info => print_info(&State::load()?)?,
        Commands::Battery { wait, watch, force } => battery_command(wait, watch, force)?,
        Commands::Tui => tui::run()?,

        Commands::Rgb(args) => {
            let effect: Effect = args.effect.parse()?;
            let p = update_lighting(profile, trace, |p| {
                p.lighting.effect = effect;
                if let Some(spec) = &args.colors {
                    let colors = parse_colors(spec)?;
                    let mut palette = [Rgb::BLACK; 7];
                    // Repeat the supplied colours so multi-slot effects never cycle to black.
                    for (i, slot) in palette.iter_mut().enumerate() {
                        *slot = colors[i % colors.len()];
                    }
                    p.lighting.colors = palette;
                    p.lighting.color_count = Some(colors.len() as u8);
                } else {
                    p.lighting.colors = DEFAULT_PALETTE;
                    p.lighting.color_count = None;
                }
                if let Some(speed) = args.speed {
                    p.lighting.speed = speed_level_from_percent(speed);
                }
                if let Some(brightness) = args.brightness {
                    let level = brightness_level_from_percent(brightness);
                    p.lighting.brightness_wired = level;
                    p.lighting.brightness_wireless = level;
                }
                Ok(())
            })?;
            println!("Effect: {}", p.lighting.effect.label());
            if !effect.is_animated() && args.speed.is_some() {
                println!(
                    "Note: '{}' does not animate; speed was stored but has no visible effect.",
                    effect.name()
                );
            }
        }

        Commands::Brightness {
            percent,
            wired_only,
            wireless_only,
        } => {
            let p = update_lighting(profile, trace, |p| {
                let level = brightness_level_from_percent(percent);
                // Default is both; the flags exist because this model reports
                // `separateBrightness: true`, so the two really are independent.
                if !wireless_only {
                    p.lighting.brightness_wired = level;
                }
                if !wired_only {
                    p.lighting.brightness_wireless = level;
                }
                Ok(())
            })?;
            println!(
                "Brightness: {}/20 wireless, {}/20 wired",
                p.lighting.brightness_wireless, p.lighting.brightness_wired
            );
        }

        Commands::Speed { percent, raw } => {
            let p = update_lighting(profile, trace, |p| {
                p.lighting.speed = match (raw, percent) {
                    (Some(v), _) => {
                        if v > SPEED_RAW_MAX {
                            bail!("raw speed must be 0-{SPEED_RAW_MAX} (0x00-0x14)");
                        }
                        v
                    }
                    (None, Some(p)) => speed_level_from_percent(p),
                    // clap's required_unless_present guarantees one of the two is set.
                    (None, None) => unreachable!("clap enforces percent or --raw"),
                };
                Ok(())
            })?;
            println!("Speed: 0x{:02x}", p.lighting.speed);
            if !p.lighting.effect.is_animated() {
                println!(
                    "Note: the active effect '{}' does not animate.",
                    p.lighting.effect.name()
                );
            }
        }

        Commands::Debounce { ms } => {
            let p = update_settings(profile, trace, |p| {
                p.settings.debounce_ms = sanitize_debounce(ms);
                Ok(())
            })?;
            println!("Debounce: {} ms", p.settings.debounce_ms);
        }

        Commands::Dpi { stages, colors } => {
            let values: Result<Vec<u16>> = stages
                .split(',')
                .map(|v| {
                    v.trim()
                        .parse::<u16>()
                        .with_context(|| format!("invalid DPI '{}'", v.trim()))
                })
                .collect();
            let values = values?;
            if values.len() < MIN_STAGES || values.len() > MAX_STAGES {
                bail!(
                    "expected {MIN_STAGES}-{MAX_STAGES} DPI stages, got {}",
                    values.len()
                );
            }
            for dpi in &values {
                validate_dpi(*dpi)?;
            }
            let parsed_colors = colors.as_deref().map(parse_colors).transpose()?;

            let p = update_settings(profile, trace, |p| {
                p.settings.stage_count = values.len() as u8;
                for (i, dpi) in values.iter().enumerate() {
                    p.settings.stage_dpis[i] = *dpi;
                }
                for slot in p.settings.stage_dpis.iter_mut().skip(values.len()) {
                    *slot = 0;
                }
                if let Some(colors) = &parsed_colors {
                    for (i, color) in colors.iter().take(MAX_STAGES).enumerate() {
                        p.settings.stage_colors[i] = *color;
                    }
                }
                if p.settings.active_stage as usize >= values.len() {
                    p.settings.active_stage = 0;
                }
                Ok(())
            })?;
            let listed: Vec<String> = (0..p.settings.stage_count as usize)
                .map(|i| p.settings.stage_dpis[i].to_string())
                .collect();
            println!("DPI stages: {}", listed.join(", "));
        }

        Commands::Stage { index } => {
            if index == 0 {
                bail!("stages are 1-based");
            }
            let p = update_settings(profile, trace, |p| {
                if index > p.settings.stage_count {
                    bail!(
                        "stage {index} is not enabled (only {} stages configured)",
                        p.settings.stage_count
                    );
                }
                p.settings.active_stage = index - 1;
                Ok(())
            })?;
            println!(
                "Active stage: {} ({} DPI)",
                p.settings.active_stage + 1,
                p.settings.active_dpi()
            );
        }

        Commands::Polling { hz } => {
            let code = polling_code_for(hz)?;
            let p = update_settings(profile, trace, |p| {
                p.settings.polling_code = code;
                Ok(())
            })?;
            println!("Polling rate: {} Hz", p.settings.polling_hz());
        }

        Commands::Lod { mm } => {
            if mm != 1 && mm != 2 {
                bail!(
                    "lift-off distance must be 1 or 2 mm (0.7 mm is not exposed by the firmware)"
                );
            }
            let p = update_settings(profile, trace, |p| {
                p.settings.lod_mm = mm;
                Ok(())
            })?;
            println!("Lift-off distance: {} mm", p.settings.lod_mm);
        }

        Commands::MotionSync { state } => {
            let enable = state == "on";
            let p = update_settings(profile, trace, |p| {
                p.settings.motion_sync = enable;
                Ok(())
            })?;
            println!(
                "Motion sync: {}",
                if p.settings.motion_sync { "on" } else { "off" }
            );
        }

        Commands::Buttons => {
            let state = State::load()?;
            let (prof, idx) = match profile {
                Some(i) => (&state.profiles[i as usize], i),
                None => (state.active(), state.active_profile),
            };
            println!("Profile {}:", idx + 1);
            for button in Button::ALL {
                println!(
                    "  {:<8} {}",
                    button.name(),
                    prof.buttons.get(button).describe()
                );
            }
        }

        Commands::Bind {
            button,
            binding,
            allow_no_left_click,
        } => {
            let button: Button = button.parse()?;
            let binding: Binding = binding.parse()?;
            let p = update_buttons(profile, trace, |p| {
                p.buttons.set(button, binding);
                if !allow_no_left_click {
                    p.buttons.validate()?;
                }
                Ok(())
            })?;
            println!("{} -> {}", button.name(), p.buttons.get(button).describe());
        }

        Commands::Macro { id, keys, delay } => {
            let events = match &keys {
                None => Vec::new(),
                Some(spec) => {
                    let mut events = Vec::new();
                    for name in spec.split(',') {
                        let name = name.trim();
                        let code = buttons::key_code(name)
                            .ok_or_else(|| anyhow::anyhow!("unknown key '{name}'"))?;
                        // Each key is a press then a release, matching how CORE records.
                        events.push(MacroEvent {
                            code,
                            press: true,
                            delay_ms: delay,
                        });
                        events.push(MacroEvent {
                            code,
                            press: false,
                            delay_ms: delay,
                        });
                    }
                    events
                }
            };
            let recorded = events.len();

            let mut state = State::load()?;
            let (prof, idx) = state.profile_mut(profile)?;
            if prof.macros.len() <= id as usize {
                prof.macros.resize(id as usize + 1, Macro::default());
            }
            prof.macros[id as usize] = Macro { events };
            state.normalize();
            let mac = state.profiles[idx as usize].macros[id as usize].clone();
            let frags = mac.fragments(id);
            let refs: Vec<&[u8]> = frags.iter().map(|f| f.as_slice()).collect();
            device::apply("macro", &refs, trace)?;
            state.save()?;

            if recorded == 0 {
                println!("Cleared macro {id} in profile {}.", idx + 1);
            } else {
                println!(
                    "Macro {id} in profile {}: {} events ({} keys).",
                    idx + 1,
                    recorded,
                    recorded / 2
                );
            }
        }

        Commands::Profile { index } => match index {
            None => {
                let state = State::load()?;
                for (i, prof) in state.profiles.iter().enumerate() {
                    let marker = if i == state.active_profile as usize {
                        "*"
                    } else {
                        " "
                    };
                    println!(
                        "{marker} {}: {} · {} DPI · {} Hz",
                        i + 1,
                        prof.lighting.effect.name(),
                        prof.settings.active_dpi(),
                        prof.settings.polling_hz()
                    );
                }
            }
            Some(index) => {
                let mut state = State::load()?;
                let idx = index - 1;
                state.active_profile = idx;
                state.normalize();
                device::switch_profile(idx, trace)?;
                // Re-push the profile's configuration so the device cannot hold a stale
                // copy from before this cache existed.
                apply_lighting(&state.profiles[idx as usize].lighting, idx, trace)?;
                apply_settings(&state.profiles[idx as usize].settings, idx, trace)?;
                apply_buttons(&state.profiles[idx as usize].buttons, idx, trace)?;
                state.save()?;
                println!("Active profile: {}", index);
            }
        },

        Commands::Sync => {
            let state = State::load()?;
            let idx = state.active_profile;
            device::switch_profile(idx, trace)?;
            apply_lighting(&state.active().lighting, idx, trace)?;
            apply_settings(&state.active().settings, idx, trace)?;
            apply_buttons(&state.active().buttons, idx, trace)?;
            println!("Re-sent profile {} to the mouse.", idx + 1);
        }

        Commands::Reset => {
            let state = State::default();
            // Restore every profile, not just the active one, so "factory defaults" means
            // what it says on a device that stores three.
            for (idx, prof) in state.profiles.iter().enumerate() {
                apply_lighting(&prof.lighting, idx as u8, trace)?;
                apply_settings(&prof.settings, idx as u8, trace)?;
                apply_buttons(&prof.buttons, idx as u8, trace)?;
            }
            device::switch_profile(state.active_profile, trace)?;
            state.save()?;
            println!(
                "Reset all {} profiles to factory defaults.",
                protocol::PROFILE_COUNT
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_are_validated() {
        assert_eq!(parse_colors("ff0000").unwrap().len(), 1);
        assert_eq!(parse_colors("f00,0f0,00f").unwrap().len(), 3);
        assert!(parse_colors("1,2,3,4,5,6,7,8").is_err(), "more than seven");
        assert!(parse_colors("nope").is_err());
    }
}
