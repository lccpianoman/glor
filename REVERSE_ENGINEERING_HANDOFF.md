# glor reverse-engineering handoff (Pixart O2/I2)

Context for the next session. Hardware: **Glorious Model O 2 Wireless receiver**, USB
`093a:822d`, `bcdDevice 2.59`, on NixOS.

## Status

The protocol is **fully mapped and implemented**, derived from Glorious CORE's own
JavaScript rather than guesswork. `PROTOCOL.md` is the byte-level reference; this file is
the narrative of how it was found, including the wrong turns.

| Feature | State |
|---|---|
| Device detect / interface selection | done |
| RGB effects (all 9), 7-colour palettes | done |
| Speed and brightness (0-20, wired + wireless separately) | done |
| DPI stages (4-6) + per-stage colours, active stage | done |
| Polling rate, lift-off, debounce, motion sync | done |
| Onboard profiles (3) + profile switch | done |
| Button bindings (6 buttons, all binding types) | done |
| Macros incl. toggle mode | done |
| Battery | done — push-only on input report `0x06`, ~65 s broadcast |
| Non-clobbering partial updates | done — via state cache |
| Inter-report pacing | done — 150 ms/fragment, 550 ms after a profile switch |
| Packet tracing (`--trace`) | done |

Not implemented, deliberately:

- **Wired mode (`0xfa`)** — a different framing, not just a different magic byte: one packet
  with data at offset 4, and DPI stages at offset 12 with an 8-byte stride.
- **Launch-program / launch-website bindings** — the device only emits a "shortcut pressed"
  event (`0x06`, `data[1] == 0xf9`); the launching is host-side, so without a daemon the
  binding would be a silent no-op.
- **Firmware update** — this is where bricking lives.

## What was resolved this session

### The firmware is write-only — proven locally, not assumed

Interface 1's report descriptor declares report `0x03` as a **Feature** report
(`B1 02`, 63 bytes), so `GET_FEATURE` is legal and does succeed. But it always returns the
same 42-byte idle response:

```
03 65 00 25 00 00 ... (38 more zero bytes)
```

Verified inert against: repeated reads, reads after lighting/settings writes, a sweep of
all 256 feature report IDs on both interfaces, and read-style command variants
(`03 {82,84,02,04,01,03,05,06,83,85} fb 00 {00,01,02}`). Every one returns the same bytes.
Control devices (Shure MV6, Telink keyboard) return `Broken pipe` for report 3, so the
response is genuinely from the mouse, not a kernel artifact.

Consequence: state must be cached locally. `src/state.rs` does this
(`~/.config/glor/state.json`, `$GLOR_STATE` to override).

### The settings payload is all-or-nothing — this was silently destroying config

`build_debounce_fragments()` used to hardcode DPI 400/800/1600/3200, polling 1000 Hz and
LOD 1 mm. Every `glor debounce N` therefore reset all of those. Fixed by load-modify-write
against the state cache; there is a regression test
(`state::tests::changing_one_setting_preserves_the_others`) and it was verified on the wire:
changing debounce moved byte [8] `0a → 04` and left the DPI bytes at `10/20/40/80`.

### The palette bug

`palette()` filled slot 0 and left six black, while byte [8] declared 7 colours for
`glorious`/`cycle`/`pulse`/`tail`/`wave`. So multi-colour effects cycled red → black × 6.
Now ships the real rainbow. This was suspected as the cause of "speed does nothing"; the
real cause turned out to be the byte assignment, below.

### SOLVED: battery is push-only on input report 0x06

Every reference implementation reports this family as having no battery support — OpenMouse
hardcodes `batteryPercent: null`, `gloriousctl-linux` omits it, the Python tool returns
"(battery query unsupported on this model)". **They are wrong.** It works; it just is not a
request/response, which is why every attempt to *ask* for it failed.

The mouse **broadcasts** an unsolicited interrupt-IN report on **input report `0x06`**,
interface 1, roughly every **65 seconds** — and additionally on a power-state change:

```text
06 fb 35 00 00 00 00 00   ->  0x35 = 53%, discharging
06 fb 35 01 00 00 00 00   ->  53%, charging
06 00 00 00 00 00 00 00   ->  idle frame, no reading
```

- [1] = `0xfb`, the same magic as the `xx fb` config commands. Frames without it carry no
  reading.
- [2] = battery percent.
- [3] = charging flag.

**It cannot be polled.** `GET_INPUT_REPORT(0x06)` times out on both interfaces. A reader has
to wait for the next broadcast.

Measured interval, from a 240 s capture with the mouse otherwise idle:

```text
[  46.5s] 06 fb 32 00 ...
[ 111.8s] 06 fb 32 00 ...     <- 65.3s later
[ 177.1s] 06 fb 32 00 ...     <- 65.3s later
```

Each battery frame is immediately followed by an idle `06 00 00 …` frame. The value tracked
real discharge across the session (0x35 = 53% -> 0x32 = 50%), confirming the decode.

Implemented as `glor battery` (waits one heartbeat plus slack, with a progress counter) and
`glor battery --watch`. Readings are cached in the state file with a timestamp and always
displayed with their age, so a stale value is never shown as current.
`device::BATTERY_HEARTBEAT` is the single source of truth for the interval.

#### Why three separate mistakes hid this

1. The first passive listen ran for **40 seconds**. The heartbeat is **65**. It found
   nothing and the conclusion drawn was "no battery support exists" — far stronger than the
   evidence supported.
2. The listening tool deduplicated by report shape, so repeated identical broadcasts after the
   first were silently suppressed and looked like the device being "inconsistent".
3. The command sweep's detector polled `GET_FEATURE(3)` and `GET_INPUT(8)` but never
   listened on the interrupt endpoint — the one channel that actually carries the data. A
   command that triggered a battery push would not have registered.

The general lesson: on a push-only device, absence of evidence over a short window is not
evidence of absence. Listen for at least two heartbeat intervals before concluding a channel
is dead.

#### No on-demand read exists (yet)

Ruled out for triggering a battery push on demand:

- `GET_INPUT_REPORT(0x06)` — times out on both interfaces.
- `GET_FEATURE(0x03)` — constant `03 65 00 25 …` regardless.
- **Sending a valid `02 fb` lighting payload** — no push follows within 3 s, tested over
  three rounds. Config writes do not provoke telemetry.

**Settled by CORE's source: no battery poll exists for this family.** The Pixart handler
(`MouseV2DeviceHandler`) defines no `getBatteryStats` at all — it only reacts to the pushed
report. The families that *do* poll bail out on wireless anyway:

```js
if (device.activeConnectionMethods.includes("Reciever")
 || device.activeConnectionMethods.includes("Bluetooth")) {
  return void 0;
}
```

So CORE cannot read battery on demand over a receiver either. A cached reading younger than
one broadcast interval is exactly as current as the official software manages, which is what
`glor battery` returns.

A repeat call returns in ~2 ms from cache. `glor battery --watch` runs as a daemon to keep
that cache warm; see the README for a systemd user unit.

#### Dead ends, so they are not retried

- Classic Sinowealth command (`bfr[3]=02, bfr[4]=02, bfr[6]=83`): no Pixart analogue responds.
- `GET_FEATURE(0x03)`: constant `03 65 00 25 …` regardless of any command sent.
- Input report `0x08`: undeclared in the descriptor but *does* answer `GET_INPUT_REPORT` on
  both interfaces, always 64 zero bytes. Control devices return nothing for it, so it is real
  — purpose unknown, no data observed yet. Worth re-checking during a firmware update or
  pairing event.
- Input report `0x01` (movement) carries a 5-byte vendor field at [7..11], constant
  `03 00 00 00 00`; does not track charge.

## Protocol reference

Feature report `0x03` on **USB interface 1** (vendor usage page `0xff00`, `/dev/hidraw2`
here). Every fragment: `03 <cmd> fb <fragment_index> 01 ...`, 64 bytes.

### Lighting — `cmd 02 fb`, 3 fragments (192 B)

| Byte | Meaning |
|---|---|
| [5] | effect id (repeated in all 3 fragments; desynced payloads are rejected) |
| [6] | **animation speed, 0x00-0x14** (references call this wireless brightness) |
| [7] | inert here (references call this wired brightness) |
| [8] | colour count (1 solid / 2 rave / 7 rainbow) |
| [9] | inert here (references call this speed) |
| [10] | **brightness, 0x00-0x14** (references call this "modifier") |
| [11..13] | primary colour RGB |
| frag 1 [3+i*3] | palette colours 1-6 |

Effect ids: `off=00 glorious=01 seamless_breathing=02 breathing=03 normally_on/solid=04
breathing_single=05 tail=06 rave=07 wave=08`.

### Settings — `cmd 04 fb`, 4 fragments (256 B)

| Byte | Meaning |
|---|---|
| [5] | active DPI stage, zero-indexed |
| [6] | total stages, 4-6 |
| [7] | LOD: 1 = 1 mm, 2 = 2 mm (0.7 mm not exposed) |
| [8] | debounce ms, even, 0x00-0x10 |
| [9] | polling: 01=1000 02=125 03=250 04=500 |
| [10] | 0x00 |

Stage slots — each is a little-endian `dpi/50` u16 followed by RGB:
`(frag 0, off 11)`, `(1, 5)`, `(1, 10)`, `(2, 5)`, `(2, 10)`, `(3, 5)`.

All offsets are asserted by unit tests in `src/protocol.rs`.

## SOLVED: speed is byte [6], brightness is byte [10]

The published docs are wrong for this firmware. Established by bisecting individual bytes
on the real device (alternate each offset `0x05` <-> `0x14`, observe
the LEDs, `wave` effect so nothing dips to black):

| Byte | `gloriousctl.c` / OpenMouse call it | Observed on `093a:822d` |
|------|-------------------------------------|-------------------------|
| [6]  | wireless brightness                 | **animation speed**     |
| [7]  | wired brightness                    | no observable effect    |
| [9]  | speed                               | no observable effect    |
| [10] | "modifier", pinned to `0x14`        | **brightness**          |

This fully explains the original "speed does nothing" blocker. Speed was written to [9],
which is inert — while the real speed byte [6] was pinned at `0x14` (maximum) by the
`brightness_wireless` default. Speed was permanently stuck at fastest. The brightness
controls were dead for the mirror-image reason.

### Resolved in full by CORE's source

The hardware bisect got the right answer; CORE's `PreparePresetEffectBuffers` then explained
the two bytes that had looked inert:

- **[7] is the *wired* brightness.** It was dormant during the bisect only because the test
  ran on a receiver. It comes alive over USB.
- **[9] is the speed, duplicated.** CORE writes `rate` to both `dataBuffer[0]` and `[3]`,
  which land on [6] and [9]. Varying [9] alone did nothing because [6] still carried the
  real value.

Speed direction confirmed on hardware: **higher is faster**. Guarded by
`protocol::tests::lighting_scalars_land_on_cores_offsets`.

Worth upstreaming to `zeppybabe/gloriousctl-linux` and `OpenMouse-Project/mouse-protocol`,
ideally after a second device confirms it. Three corrections are worth sending: the speed
and brightness byte assignments, the polling-rate code table, and motion sync on byte [10].

## Environment notes

- hidraw nodes are root-only by default. `./run-temp.sh` chmods them for one command.
  A permanent udev rule is in the README (NixOS `services.udev.extraRules`).
- `nix develop -c cargo …` for builds; `nix-shell -p <tool>` for ad-hoc tools.
- The raw-HID scratch tooling used during reverse engineering has been removed now that
  CORE's source is the reference. Recover it from git history if ever needed.

## ⚠️ INCIDENT: a partial command sweep unbound the mouse buttons

During the battery hunt on 2026-09-07 this sweep was run against the live device:

```
03 {82,84,02,04,01,03,05,06,83,85} fb 00 {00,01,02} + 59 zero bytes
```

Shortly afterwards **all mouse buttons stopped responding while the sensor kept working**.
Commands `01`, `03`, `05` and `06` were sent with no known layout and an all-zero body; one
of them is very likely the button/keybind map, which was therefore written as "everything
unbound". The same sweep also sent `03 04 fb 00 01` with a zero body (stage_count=0,
polling=0), though later valid settings writes overwrote that.

Recovery: the documented **hardware** factory reset — hold left + right + scroll click for
5 s until the LEDs flash green. It is handled by firmware reading the physical switches, so
it works even with the button map zeroed.

**Do not send vendor commands whose payload layout is unknown.** There is no readback, so
there is no way to snapshot state before experimenting and no way to verify afterwards. The
only safe experiments are on the two commands whose layout is fully mapped (`02 fb`
lighting, `04 fb` settings).

### Open question left by the incident

It is not yet confirmed *which* write broke the buttons. Two candidates:

1. **An unknown sweep command** (`01`/`03`/`05`/`06` `fb`) — most likely, since
   `gloriousctl-linux` and OpenMouse both ship the `04 fb` settings payload to real users
   without reports of dead buttons.
2. **The `04 fb` settings payload itself**, if button data lives in the bytes those
   implementations label "padding" and we write as zero.

To distinguish, after a successful factory reset run a single settings write
(`glor debounce 10`) and re-test the buttons. If they survive, candidate 1 is confirmed and
the settings payload is safe. If they die again, `glor` must stop zeroing the padding and
the button region must be mapped before the settings payload is used again.

## Reference implementations

- <https://github.com/zeppybabe/gloriousctl-linux> — `gloriousctl.c` has the payload structs
  with byte-level comments. Closest match to this hardware.
- <https://github.com/OpenMouse-Project/mouse-protocol> — `src/glorious/index.ts` (codec) and
  `src/drivers/glorious/hid.ts` (driver). Confirms write-only + no battery.
- `~/Code/glorious-ctl` — the Python tool, validated on this exact mouse.
