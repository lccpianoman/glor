# glor

Nix-first CLI/TUI for Pixart-based Glorious mice on Linux — the **Model O 2 / I 2** family
(USB vendor `093a`), which the older Sinowealth-era tools (`gloriousctl`, `mxw`, libratbag)
do not support.

Developed and tested against a **Model O 2 Wireless** receiver (`093a:822d`).

## Install / run

```bash
nix run github:you/glor -- info      # or, in a checkout:
nix develop -c cargo run -- info
./run-temp.sh info                   # wrapper that also fixes hidraw permissions
```

Requires nixpkgs ≥ 25.11. Older channels vendor crates from
`crates.io/api/v1/crates/…/download`, which now returns 403 to Nix's sandboxed fetcher;
newer ones use `static.crates.io`. Also note that `nix build` only sees **git-tracked**
files — `git add` new source files or the build will fail on missing modules.

## Commands

```bash
glor detect                          # show the connected mouse
glor doctor                          # diagnose interfaces + hidraw permissions
glor info                            # show the cached configuration
glor battery                         # read battery (waits for the ~65s broadcast)
glor tui                             # interactive UI

glor rgb breathing                                   # effect, default rainbow palette
glor rgb solid --colors ff0000                       # solid red
glor rgb rave --colors ff0000,0000ff                 # rave takes two colours
glor rgb wave --speed 100 --brightness 60
glor speed 75                                        # 0-100, mapped onto the 0-20 scale
glor speed --raw 12                                  # exact 0-20 byte
glor brightness 40                                   # both wired and wireless
glor brightness 10 --wireless-only                   # this model keeps them separate

glor dpi 800,1600,3200,6400                          # 4-6 stages, multiples of 50
glor dpi 400,800,1600,3200 --colors f00,00f,0f0,ff0  # with stage indicator colours
glor stage 2                                         # select active stage
glor polling 1000                                    # 125 / 250 / 500 / 1000
glor debounce 4                                      # even, 0-16 ms
glor lod 1                                           # 1 or 2 mm

glor motion-sync on                                  # motion sync

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
glor --trace <any command>                           # hexdump outgoing fragments
```

`glor bind` refuses a mapping that leaves no button bound to left-click — the firmware
accepts it happily, and the result is a mouse you cannot use to undo the mistake without the
hardware factory reset (hold left + right + scroll click for 5 s). Pass
`--allow-no-left-click` if you really mean it.

## How it works

The configuration interface is HID **feature report `0x03`** on USB interface 1
(vendor usage page `0xff00`). Two payloads exist, each split into 64-byte fragments:

| Payload  | Command   | Fragments | Carries |
|----------|-----------|-----------|---------|
| Lighting | `02 fb`   | 3 (192 B) | effect, brightness, colour count, speed, 7-colour palette |
| Settings | `04 fb`   | 4 (256 B) | active stage, stage count, LOD, debounce, polling, 6 × (DPI + colour) |

Every fragment begins `03 <cmd> <fb> <index> 01`. Fragment layouts are documented in
`src/protocol.rs`, with unit tests asserting each field's byte offset.

### Correction to the published protocol docs

Every public description of this protocol puts **speed and brightness on the wrong bytes**.
Confirmed against CORE's own `PreparePresetEffectBuffers`, and by bisecting each byte on a
Model O 2 Wireless (`093a:822d`, `bcdDevice 2.59`):

| Byte | `gloriousctl.c` / OpenMouse call it | What it actually does |
|------|-------------------------------------|-----------------------|
| [6]  | wireless brightness                 | **animation speed**   |
| [7]  | wired brightness                    | **wired** brightness — dormant on a receiver |
| [9]  | speed                               | speed, duplicated; inert on its own |
| [10] | "modifier", pinned to `0x14`        | **wireless** brightness |

This is why every tool for this mouse has a speed slider that does nothing: speed is written
to [9], which is inert, while the real speed byte [6] sits pinned at `0x14` — maximum — as
"wireless brightness". Brightness controls are equally dead for the same reason.

`glor` writes the confirmed bytes *and* their documented counterparts, so the payload stays
correct if another firmware revision uses the layout the references describe. The
counterpart bytes were verified inert across the whole `0x05`-`0x14` range.

### The firmware is write-only

It accepts configuration but never reports it back. `GET_FEATURE(0x03)` answers with a
constant 42-byte idle response (`03 65 00 25 00…`) regardless of what was last written, and
no read-style command variant produces anything else. This matches every other
implementation of this protocol.

Two consequences:

1. **`glor` mirrors your configuration** in `~/.config/glor/state.json` (override with
   `$GLOR_STATE`). Both payloads are all-or-nothing, so changing debounce means rewriting
   DPI, polling, LOD and every stage colour too. Without the mirror, setting one value
   silently resets the others — which is exactly what earlier versions of this tool did.
2. **Battery is readable, but only as an event.** See below.

If the state file and the mouse ever disagree (e.g. you used Glorious CORE on Windows),
`glor sync` re-imposes the cached configuration.

### Battery

Every other implementation of this protocol reports the Model O 2 / I 2 family as having no
battery support — OpenMouse hardcodes `batteryPercent: null`, and neither `gloriousctl-linux`
nor the Python tool implements it. It does work; it is just not a request/response.

The mouse **broadcasts** battery on HID **input report `0x06`** (interface 1) roughly every
**65 seconds**, unprompted, and additionally on a power-state change. It cannot be polled —
`GET_INPUT_REPORT` on that ID times out — so a reader has to wait for the next broadcast.

The heartbeat is why this looked impossible for so long: measured at 65.3 s between pushes,
it is just long enough that a 40-second listen sees nothing at all and concludes the device
has no battery support.

```text
06 fb 35 00 00 00 00 00   ->  0x35 = 53%, discharging
06 fb 35 01 00 00 00 00   ->  53%, charging
06 00 00 00 00 00 00 00   ->  idle frame, no reading
```

Byte [1] is `0xfb`, the same magic as the `xx fb` config commands; [2] is the percentage and
[3] the charging flag.

```bash
glor battery              # instant if the cache is current, else waits for a broadcast
glor battery --force      # ignore the cache and wait for a fresh one
glor battery --watch      # run as a daemon: listens forever, keeping the cache warm
```

**There is no on-demand read.** The mouse cannot be asked; it only broadcasts. Ruled out so
far: `GET_INPUT_REPORT(0x06)` (times out), `GET_FEATURE(0x03)` (constant idle value), and
sending a normal `02 fb` config write (no push follows).

What `glor` does instead is treat a cached reading younger than one broadcast interval as
current — because it *is*: the mouse has not sent anything newer, so waiting cannot produce
a better answer. That makes a repeat call instant:

```console
$ glor battery
Battery: 50% (as of 16s ago)      # 2 ms
```

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

```bash
systemctl --user enable --now glor-battery
```

`glor info` and the TUI header always show the reading's age, so a stale value is never
presented as current.

## Permissions

hidraw nodes are root-only by default. `./run-temp.sh` chmods them for the duration of one
command. For a permanent fix on NixOS:

```nix
services.udev.extraRules = ''
  KERNEL=="hidraw*", ATTRS{idVendor}=="093a", MODE="0660", TAG+="uaccess"
'';
```

On other distros, put the same line in `/etc/udev/rules.d/70-glorious.rules` and run
`sudo udevadm control --reload && sudo udevadm trigger`.

## Effects

`off`, `glorious`, `seamless-breathing`, `breathing`, `solid`, `breathing-single`, `tail`,
`rave`, `wave`. Older `glor`/`glorious-ctl` spellings (`cycle`, `pulse`, `pulse-one`) still
resolve.

`glorious` is the firmware's built-in rainbow and ignores your palette. `solid` and
`breathing-single` use one colour, `rave` two, the rest cycle all seven.

## Protocol

Every payload is derived from **Glorious CORE's own source**, not from guesswork: CORE is an
Electron app whose `app.asar` unpacks to readable JavaScript. `PROTOCOL.md` documents the
full command set, byte layouts, timing requirements and the extraction recipe.

Three things in there contradict the other open-source implementations of this protocol
([gloriousctl-linux](https://github.com/zeppybabe/gloriousctl-linux),
[OpenMouse](https://github.com/OpenMouse-Project/mouse-protocol)), and all three are why
those tools misbehave on this hardware:

- **Speed is byte [6], not [9].** Writing it to [9] leaves the real field pinned at maximum,
  which is why speed sliders appear to do nothing.
- **Polling codes are `125→1, 250→2, 500→3, 1000→4`.** The published table is a rotation of
  this, so asking for 1000 Hz actually selects 125 Hz.
- **Byte [10] of the settings payload is motion sync**, not padding. Hardcoding it to zero
  silently disables the feature on every write.

`REVERSE_ENGINEERING_HANDOFF.md` records how each of these was found, including the mistakes
made along the way.
