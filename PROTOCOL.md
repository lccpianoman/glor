# Pixart Glorious protocol (Model O 2 / I 2 family, VID `093a`)

Derived from **Glorious CORE 2.1.21's own source**, plus hardware verification on a Model O 2
Wireless receiver (`093a:822d`).

CORE is an Electron app. The Windows installer (Inno Setup) unpacks to `resources/app.asar`,
which extracts to readable bundled JavaScript. The mouse driver is
`MouseV2DeviceHandler` and its payload builders in `out/main/index.js`. **The HID protocol is
implemented in JavaScript, not in the bundled native DLLs** — `DllSDK/model_i_dll.dll` serves
the older non-Pixart Model I (`22d4:1503`) and is irrelevant here.

This supersedes the guesses in `gloriousctl-linux` and OpenMouse, both of which have the
lighting byte layout wrong (see below).

---

## Transport

| Connection | Report ID | Byte [2] magic | Framing |
|---|---|---|---|
| 2.4 GHz receiver | `0x03` | `0xfb` | `03 <cmd> fb <fragment> <profile+1> <data…>` |
| Wired USB | `0x03` | `0xfa` | `03 <cmd> fa <profile> <data…>` — single 64-byte packet, no fragmenting |
| Bluetooth | `0x05` | *(none)* | `05 <cmd> <fragment> <profile+1> <data…>`, 21-byte buffers |

Byte [4] is **`profileIndex + 1`**, not a constant. `glor` currently hardcodes `0x01`, which
means "profile 0" and is why it works — but multiple onboard profiles exist.

> Wired framing differs by more than the magic byte: it sends the whole payload in one
> packet with data at offset 4, rather than chunking. A wired implementation is not just
> `0xfb` → `0xfa`.

## Timing — reports must be paced

CORE waits between every feature report, and the firmware requires it:

```js
delay = 150;   // per report, USB or Reciever
delay = 30;    // Bluetooth
static sendReportToDevice(device, report, delayAfter = 550)   // profile switch
```

| Between | Delay |
|---|---|
| Consecutive fragments (receiver / wired) | **150 ms** |
| Consecutive fragments (Bluetooth) | 30 ms |
| After a profile switch, before anything else | **550 ms** |

Each report is relayed over the 2.4 GHz link and written to flash. A report that arrives
while the previous one is still being processed is **silently dropped** — no error, no
NAK. Sending back-to-back appears to work for a single payload, then loses writes as soon
as several are sent in a row.

The failure this caused here: switching profile and immediately pushing that profile's
lighting meant the lighting payload landed mid-switch and was discarded. The mouse kept its
own stored effect and brightness while the local cache believed the write had succeeded —
so brightness appeared "stuck" across a profile change.

A profile switch reloads a whole configuration from flash, which is why it needs far longer
than an ordinary fragment.

## Commands (byte [1])

| Cmd | Purpose | Fragments (receiver) | Data offset |
|---|---|---|---|
| `0x02` | Lighting | 3 | byte 6 (after effect id at byte 5) |
| `0x03` | **Button bindings** | 4+ | byte 5, 12 bytes per fragment |
| `0x04` | Settings (DPI/polling/LOD/debounce) | 4 | byte 5, 11 then 10 bytes |
| `0x05` | **Macros** | n | `03 05 fb <macroID> 00 00 <eventCount>` |

`0x03` and `0x05` are the two families `glor` does not implement, and are exactly the
commands that unbound the buttons when swept blind with zero bodies.

## Lighting — `02 fb`

Packet: `03 02 fb <frag> <profile+1> <effectId> <data…>`, data at byte 6.

CORE builds an intermediate `dataBuffer`, then copies it into the fragments:

```js
dataBuffer[0] = rate;             // speed
dataBuffer[1] = wiredBright;
dataBuffer[2] = ColorCount;
dataBuffer[3] = rate;             // speed, duplicated
dataBuffer[4] = wirelessBright;
dataBuffer[5 + i*3 ..] = colors;  // receiver/BT layout
```

Fragment mapping (`buffer1[6+i] = dataBuffer[i]`), giving absolute packet offsets:

| Byte | Field | Notes |
|---|---|---|
| [5] | effect id | repeated in every fragment |
| **[6]** | **speed** | 0–20. Verified on hardware. |
| [7] | **wired** brightness | 0–20. Inert on a receiver — only applies when plugged in. |
| [8] | colour count | actual number of colours supplied, not a per-effect constant |
| [9] | speed (duplicate) | inert on its own, because [6] carries the same value |
| **[10]** | **wireless** brightness | 0–20. Verified on hardware. |
| [11..13] | colour 0 RGB | |

Remaining colours, receiver layout:

- fragment 1 bytes [6..15) ← `dataBuffer[8..17)` = colours 1, 2, 3
- fragment 2 bytes [6..15) ← `dataBuffer[17..26)` = colours 4, 5, 6

> ⚠️ `glor` currently packs colours 1-6 contiguously into fragment 1 at `3 + i*3`. Colours
> 4-6 belong in **fragment 2**. See `TODO` in `protocol.rs`.

### Why the published docs are wrong

`gloriousctl-linux` calls [6] "wireless brightness", [7] "wired brightness", [9] "speed" and
[10] a "modifier" pinned to `0x14`. OpenMouse copies that. The result: speed is written to
the inert duplicate [9] while the real speed byte [6] sits pinned at `0x14` (maximum), so
**speed appears to do nothing** — the exact symptom this project started with. Brightness
lands on [7], which is inert on a receiver, so it appears dead too.

### Scale

```js
rate       = speed      > 4 ? speed      * 20 / 100 : 1;
brightness = brightness > 4 ? brightness * 20 / 100 : (brightness == 0 ? 0 : 1);
```

Both are **continuous 0–20**, mapped from a 0–100 UI percentage. The four "speed levels"
(`05 0a 0f 14`) in every existing implementation are an invention; the firmware accepts the
whole range.

### Effect ids

`off=00 glorious=01 seamless_breathing=02 breathing=03 normally_on=04 breathing_single=05
tail=06 rave=07 wave=08` — as previously known.

## Settings — `04 fb`

```js
const RCV_HEADER_DATA = Buffer.from([3, 4, 251, 0, hwProfileIndex]);
const RCV_BUFFER_INDEX = 3;   // byte[3] = fragment index
const RCV_DATA_OFFSET  = 5;   // payload starts at byte 5
const RCV_DATA_LEN     = 10;
const RCV_DATA_CHUNKS  = 4;
```

Fragment 0 carries 11 payload bytes, fragments 1-3 carry 10 each → **41 bytes of profile
data** total. Byte [4] is `profileIndex + 1`, the same as every other payload
(`hwProfileIndex = profileIndex + 1`, used directly in the header).

`dataBuffer` layout, straight from `PreparePerformanceBuffers2`:

| `dataBuffer` | Packet byte | Field |
|---|---|---|
| [0] | [5] | active DPI stage index |
| [1] | [6] | number of DPI stages |
| [2] | [7] | lift-off distance |
| [3] | [8] | debounce ms |
| [4] | [9] | **polling code** — see below |
| [5] | [10] | **motion sync** flag (`1`/`0`) |
| [6 + 5·i] | — | DPI stage *i*: `round(dpi/50)` as int16 LE, then RGB |

Stage slots therefore land at packet `(frag 0, 11)`, `(1, 5)`, `(1, 10)`, `(2, 5)`,
`(2, 10)`, `(3, 5)` — which matches what `glor` already had. Wired mode differs: stages
start at `dataBuffer[12]` with an **8**-byte stride, not 6/5.

### Polling codes — third-party docs are wrong

```js
const arrRate      = [125, 250, 500, 1000];
const arrRateValue = [1, 2, 3, 4];
dataBuffer[4] = arrRateValue[arrRate.indexOf(pollingRate)];
```

So **125→1, 250→2, 500→3, 1000→4**. `gloriousctl-linux` documents
`0x01(1k), 0x02(125), 0x03(250), 0x04(500)` — a rotation, under which asking for 1000 Hz
actually selects 125 Hz. OpenMouse copies the same wrong table.

### Motion sync

Packet byte [10] is `MotionSyncFlag ? 1 : 0`. Every other implementation hardcodes it to
`0x00` and calls it padding, silently disabling the feature on every settings write.

## Inbound reports (interrupt IN, report ID `0x06`)

The device pushes these; they are **not** responses to anything.

| `data[1]` | Meaning | Payload |
|---|---|---|
| `0xf6` (246) | Active profile changed | `data[2]` = profile index |
| `0xf9` (249) | Button pressed (shortcut) | `data[2] >= 128`, `data[3]` = HID button id |
| `0xfb` (251) | **Battery** | `data[2]` = percent, `data[3]` ≠ 0 → charging |

CORE's own decode:

```js
} else if (data[0] == 6 && data[1] == 251) {
  if (data[2] < 0 || data[2] > 100) { /* reject */ }
  const batteryStatus = { chargeLevel: data[2], state: data[3] == 0 ? "" : "charging" };
```

### There is no battery poll — by design

`MouseV2DeviceHandler` (the Pixart family) defines **no** `getBatteryStats` or
`requestBatteryStats`. It only reacts to the pushed report. Other device families in CORE do
implement polling, and even there:

```js
static async getBatteryStats(device) {
  await this.sendReportToDevice(device, dataBuffer, 50);
  if (device.activeConnectionMethods.includes("Reciever")
   || device.activeConnectionMethods.includes("Bluetooth")) {
    return void 0;      // wireless cannot do request/response for battery
  }
  ...
}
```

So **Glorious CORE cannot read battery on demand over a receiver either.** It waits for the
same broadcast `glor` waits for (~65 s, measured). Treating a sub-heartbeat cached reading as
current is not a workaround — it is exactly as fresh as the official software gets.

`BatteryStatusCheck` also exists as a *button function* (press a button, the LEDs indicate
charge). That is firmware-side, not a host command.

## Profile switch — `01 fb`

```
03 01 fb <profileIndex+1>
```

Four bytes, no payload. (`0xfa` when wired.)

## Button bindings — `03 fb`

Two 64-byte packets for a receiver, 12 payload bytes each at offset 5:

```
03 03 fb 00 <profile+1> <keyBuffer[0..12)>
03 03 fb 01 <profile+1> <keyBuffer[12..24)>
```

The key buffer is **4 bytes per button**, indexed by `hidButtonID * 4`. A receiver sends 24
bytes = 6 buttons. CORE's factory default:

```
1,1,0,0   1,2,0,0   1,3,0,0   1,4,0,0   1,5,0,0   102,3,0,0   1,160,0,0   1,161,0,0   0,0,0,0   0,0,0,0
   L         R         M       back      fwd      DPI cycle
```

### Slot map (`hidButtonID`)

| Slot | Button | Sent over receiver? |
|---|---|---|
| 0 | Left | yes |
| 1 | Right | yes |
| 2 | Middle | yes |
| 3 | Back | yes |
| 4 | Forward | yes |
| 5 | DPI cycle | yes |
| 6 | Scroll up | no — beyond the 24 bytes |
| 7 | Scroll down | no |

The Model O 2's component layout lists exactly six configurable buttons — left, right,
middle, back, forward, DPI cycle — which is why the receiver only transmits 24 bytes.

Byte [0] of each slot is the binding *type*:

| [0] | Binding | [1] | [2] | [3] |
|---|---|---|---|---|
| `0` | Disabled | `0` | — | — |
| `1` | Mouse function | `MouseFunctionHIDMap` value | — | — |
| `2` | Keystroke | modifier mask | HID key code | — |
| `3` | Multimedia | HID map, written **backwards** into `[+2-i]` | | |
| `4` | Keyboard function / launch program / launch website | `1` | — | — |
| `102` | DPI function (shift, cycle up/down, stage up/down) | function id | dpi÷50 low | dpi÷50 high |
| `119` | Battery status check (LED indication) | | | |
| `136` | Layer shift | | | |
| `31 + macroValue` | Macro | mode | — | — |

Modifier mask is a bitfield: `bit0 Ctrl, bit1 Shift, bit2 Alt, bit3 Meta`.

Macro modes (the "button toggle" behaviour): `1` = no repeat, `224` = repeat while holding,
`225` = **toggle**.

Windows shortcuts reuse type `2` (Explorer) or `3`, with the HID map written backwards.

### Mouse function ids (byte [1] when type is `1` or `102`)

| Function | id | Function | id |
|---|---|---|---|
| None / LeftClick | 1 | ProfileCycleUp | 176 |
| RightClick | 2 | ProfileCycleDown | 177 |
| MiddleClick | 3 | BatteryStatusCheck | 0 (type `119`) |
| Back | 4 | LayerShift | 0 (type `136`) |
| Forward | 5 | DPIStageUp | 1 (type `102`) |
| ScrollUp | 160 | DPIStageDown | 2 (type `102`) |
| ScrollDown | 161 | DPICycleUp | 3 (type `102`) |
| | | DPICycleDown | 4 (type `102`) |
| | | DPIShift | 5 (type `102`) |

### Multimedia (type `3`)

Two-byte maps written **backwards** — `slot[2] = map[0]`, `slot[1] = map[1]`:

| Action | map | Action | map |
|---|---|---|---|
| Play/Pause | `00 cd` | Volume up | `00 e9` |
| Stop | `00 b7` | Volume down | `00 ea` |
| Mute | `00 e2` | Next track | `00 b5` |
| Open media player | `01 83` | Previous track | `00 b6` |

### Windows shortcuts

`Email 01 8a`, `Calculator 01 92`, `MyComputer 01 94`, `Explorer 08 08` (type `2`; CORE
notes the Model O series uses `01 94` instead), `BrowserHome 02 23`, `Refresh 02 27`,
`Back 02 24`, `Forward 02 25`, `Search 02 21`.

## Profiles

The Model O 2 has **3 onboard profiles** (`minimumProfilesCount` and
`maximumIterableProfilesCount` are both 3). Every payload — lighting, settings, buttons —
carries `profileIndex + 1` in byte [4]. Command `01 fb` switches the active profile, and the
device announces a profile change back on input report `0x06` with `data[1] == 0xf6`.

## Device identity

| Connection | VID:PID | Write collection | Read collection |
|---|---|---|---|
| Wired USB | `093a:822a` | usagePage `ff00`, usage `01` | usagePage `ff00`, usage `ff00` |
| Receiver | `093a:822d` | same | same |
| Bluetooth | `093a:822b` | same | same |

CORE uses **two different HID collections** on the vendor page: usage `0x01` for writes and
usage `0xff00` for reads. On Linux both resolve to the same hidraw node, so it makes no
practical difference here, but it explains why hidapi enumerates the interface twice.

`features` for this model include `battery: true`, `pairing: true` and
**`separateBrightness: true`** — wired and wireless brightness are independent.

## Macros — `05 fb`

Header, receiver:

```
[0]=03  [1]=05  [2]=fb  [3]=hwMacroID  [4]=pageIndex  [5]=pageCount  [6]=eventCount
data at offset 7, 3 events per page
```

`hwMacroID` is `macro.value - 1` (floored at 0). Each event is **3 bytes**:

| Byte | Meaning |
|---|---|
| 0 | `delayHigh`: bit 7 set = key **press**, clear = release; bits 0-6 = `delay >> 8` |
| 1 | `delayLow`: `delay & 0xff` |
| 2 | HID key code |

Delay is milliseconds to the *next* event, minimum 1. The macro body is built into a
248-byte buffer and paged out 3 events (9 bytes) at a time.

Wired (`0xfa`) fits 18 events per page with data at offset 8; Bluetooth uses report `0x05`,
3 per page, data at offset 6.

> This is the command my blind sweep hit. `03 05 fb 00 01` + zeros writes an empty macro;
> `03 03 fb 00 01` + zeros writes an all-zero key buffer, i.e. **every button Disabled**.
> That is exactly the observed symptom, now confirmed from source rather than inferred.

## Open work

- Colours 4-6 belong in fragment 2, not fragment 1.
- Speed/brightness should expose the full 0–20 range, not 4/5 snapped levels.
- Wired mode (`0xfa`) is a different framing, not just a different magic byte.
- Button bindings (`0x03`) and macros (`0x05`) are unimplemented; layouts are in
  `out/main/index.js` around lines 49900-50560 and 62150-62250.
- Profiles: byte [4] is a profile index; `glor` models a single profile.

## Reproducing the extraction

```bash
7z x GloriousCORE_2.1.21_Setup.zip
# innoextract fails on this Inno version; use 32-bit wine to unpack
WINEPREFIX=/tmp/p wine GloriousCORE_2.1.21_Setup.exe /VERYSILENT /DIR='Z:\path\out'
npx @electron/asar extract out/resources/app.asar asar/
# driver: asar/out/main/index.js, class MouseV2DeviceHandler
```
