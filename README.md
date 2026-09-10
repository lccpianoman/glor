# glor

Configure Pixart-based **Glorious Model O 2 / I 2** mice on Linux — RGB, DPI, polling rate,
lift-off distance, debounce, motion sync, onboard profiles, button remapping, macros and
battery.

These are the `093a` mice. The Sinowealth-era tools — `gloriousctl`, `mxw`, libratbag —
target the older `258a` generation and do not support them.

Developed against a **Model O 2 Wireless** receiver (`093a:822d`).

## Install

### NixOS (flake)

```nix
{
  inputs.glor = {
    url = "github:lccpianoman/glor";
    # Optional, but recommended: build against your nixpkgs instead of pulling a
    # second copy into the store.
    inputs.nixpkgs.follows = "nixpkgs";
  };

  # in your system configuration:
  imports = [ inputs.glor.nixosModules.default ];
  programs.glor.enable = true;
}
```

That installs the CLI and the udev rule, so the mouse is usable without elevation.

This flake tracks `nixos-26.05`, and `flake.lock` pins an exact revision for reproducible
builds. If you set `inputs.glor.inputs.nixpkgs.follows = "nixpkgs"`, it will use your own
nixpkgs instead.

### Any distribution with Nix

```bash
nix run github:lccpianoman/glor -- info
nix profile install github:lccpianoman/glor
```

### From source

Needs Rust **1.85+**, a C toolchain (`gcc` or `clang`), `pkg-config`, and libudev headers
(`libudev-dev` on Debian/Ubuntu, `systemd-devel` on Fedora):

```bash
cargo build --release
```

On NixOS, the easiest path is:

```bash
nix develop -c cargo build --release
```

## Permissions

hidraw nodes are root-only by default. Install the udev rule once and the mouse becomes
usable by the logged-in user:

```bash
glor doctor --udev-rule | sudo tee /etc/udev/rules.d/70-glorious.rules
sudo udevadm control --reload-rules && sudo udevadm trigger
```

Then replug the mouse or its receiver. The rule uses `TAG+="uaccess"`, which grants access
to whoever is physically logged in — no group management, and access follows the session.

If the rule is missing, `glor` explains how to install it and then offers to re-run the
command under `sudo` so you are not blocked. `--no-sudo` disables that. An elevated run
hands the config file back to your user afterwards, so it never becomes root-owned.

`glor doctor` diagnoses interface selection and permissions.

## Usage

```bash
glor detect                          # show the connected mouse
glor info                            # show the cached configuration
glor battery                         # battery level

glor rgb breathing                                   # effect, default rainbow palette
glor rgb solid --colors ff0000                       # solid red
glor rgb rave --colors ff0000,0000ff                 # rave takes two colours
glor rgb wave --speed 100 --brightness 60
glor speed 75                                        # 0-100, mapped onto the 0-20 scale
glor brightness 40                                   # both wired and wireless
glor brightness 10 --wireless-only                   # this model keeps them separate

glor dpi 800,1600,3200,6400                          # 4-6 stages, multiples of 50
glor dpi 400,800,1600,3200 --colors f00,00f,0f0,ff0  # with stage indicator colours
glor stage 2                                         # select active stage
glor polling 1000                                    # 125 / 250 / 500 / 1000
glor debounce 4                                      # even, 0-16 ms
glor lod 1                                           # 1 or 2 mm
glor motion-sync on

glor buttons                                         # show the button mapping
glor bind forward mute                               # media key
glor bind middle key:ctrl+c                          # keystroke with modifiers
glor bind back dpi-shift:400                         # hold for 400 DPI
glor macro 1 ctrl,c                                  # record a macro into slot 1
glor bind middle macro:1:toggle                      # press once to start, again to stop

glor profile                                         # list the 3 onboard profiles
glor profile 2                                       # switch active profile
glor --profile 3 rgb wave                            # edit profile 3 without switching

glor sync                                            # re-push the cached config
glor reset                                           # factory defaults, all profiles
glor --trace <command>                               # hexdump outgoing fragments
```

`glor bind` refuses a mapping that leaves no button bound to left-click — the firmware
accepts it, and the result is a mouse you cannot use to undo the mistake without the
hardware factory reset (hold left + right + scroll click for 5 s). `--allow-no-left-click`
overrides.

### Effects

`off`, `glorious`, `seamless-breathing`, `breathing`, `solid`, `breathing-single`, `tail`,
`rave`, `wave`.

`glorious` is the firmware's built-in rainbow and ignores your palette. `solid` and
`breathing-single` use one colour, `rave` two, the rest cycle all seven.

### Battery

The mouse **broadcasts** battery roughly every 65 seconds and cannot be polled — not by
`glor`, and not by Glorious CORE either, which waits for the same broadcast. `glor battery`
returns instantly from cache when the last reading is younger than one interval, and
otherwise waits for the next one.

To keep it always instant, run the listener as a user service:

```ini
# ~/.config/systemd/user/glor-battery.service
[Unit]
Description=glor battery listener
[Service]
ExecStart=%h/.nix-profile/bin/glor battery --watch
Restart=always
[Install]
WantedBy=default.target
```

Readings are always shown with their age, so a stale value is never presented as current.

## How it works

The configuration interface is HID **feature report `0x03`** on USB interface 1. The
firmware is **write-only**: it accepts configuration but never reports it back. `glor`
therefore mirrors your configuration in `~/.config/glor/state.json` and rewrites the full
payload on every change, so adjusting one setting does not reset the others.

If the cache and the mouse ever disagree — say you used Glorious CORE on Windows —
`glor sync` re-imposes the cached configuration.

Every payload is derived from **Glorious CORE's own source**: CORE is an Electron app whose
`app.asar` unpacks to readable JavaScript. [`PROTOCOL.md`](PROTOCOL.md) documents the
complete command set, byte layouts and timing requirements, with the extraction recipe.

Four findings there contradict every other open-source implementation of this protocol, and
each causes real misbehaviour:

- **Speed is byte [6], not [9].** Writing it to [9] leaves the real field pinned at maximum,
  which is why speed sliders appear to do nothing.
- **Polling codes are `125→1, 250→2, 500→3, 1000→4`.** The published table is a rotation, so
  asking for 1000 Hz actually selects 125 Hz.
- **Settings byte [10] is motion sync**, not padding. Hardcoding it to zero silently
  disables the feature on every write.
- **Reports must be paced** — 150 ms between fragments, 550 ms after a profile switch.
  Unpaced writes are silently dropped by the firmware.

## Status

A TUI existed during development but is not shipped: it covered only part of the settings
and did not fit an 80×24 terminal. It is in the git history and may return.

Not implemented: wired-mode framing (`0xfa` uses a different layout, not just a different
magic byte), launch-program/website bindings (the device only emits an event; the launching
is host-side), and firmware updates.

## Licence

MIT. See [LICENSE](LICENSE).

Reverse engineered for interoperability. This is an independent implementation; it
redistributes no Glorious code and is not affiliated with or endorsed by Glorious.
