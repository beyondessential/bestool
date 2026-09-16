# bliti implementation plan

Behaviour is specified under `.workhorse/specs/bliti/`.
This plan carries the implementation choices, what was weighed against them, and the outstanding work.
The reasoning behind the design, including the measurements it rests on, is in `.workhorse/working-docs/u2/working-doc.md`.

## Crate layout

Two crates, both in this repository, standing alone from `bestool` — its own binary rather than a subcommand, with nothing in `bestool` depending on it, which keeps moving it to its own repository later a cheap option.

- **Protocol core.** Board ID reading, the key schedule, the handshake, framing, and message types. No BlueZ and no hardware, so it unit-tests anywhere.
- **Daemon.** Ties the core to `bluer`, plus sticker generation.

Board ID reading sits in the core as backends behind a trait — TPM, one-time-programmable memory, Raspberry Pi serial, SMBIOS, and a test backend — rather than as its own crate. It can move out if something else needs it.

The core earns its separation because the web application needs the same handshake in the browser. Compiling it to wasm keeps one implementation of the key schedule rather than a Rust one and a JavaScript one that must agree forever. This constrains the core to stay free of I/O and of anything that will not build for `wasm32-unknown-unknown`.

Note that argon2 never runs in a browser: the client reads the sticker secret from the QR and only computes the handle, which is a fast hash. The memory-hard derivation is needed by the device and the generator only, and is feature-gated so a wasm build does not pull it.

## Dependencies

- **`bluer`** for the BlueZ layer — the BlueZ project's own Rust interface, BSD-2-Clause so compatible with our GPL-3.0-or-later, covering peripheral advertising and the GATT server, and shipping L2CAP behind a feature flag. Means no BlueZ plumbing on our maintenance surface.
- **`snow`** for Noise, whose pure-Rust resolver builds for wasm.
- **`argon2`** for the first derivation, with the `parallel` feature where it is available. Verified that the feature does not change the digest, so it is purely a speed choice.
- **`yamux`** for the stream layer. Verified to build for `wasm32-unknown-unknown`; it depends unconditionally on `web-time`, so browser support is deliberate upstream.
- Reaching a TPM needs either the established bindings to the TPM software stack, which carry a C library, or speaking the TPM command protocol to the resource manager directly. The operation needed is narrow — create a primary key under the endorsement hierarchy with the pinned template, read its name — so both are viable. Whichever is chosen belongs behind the board-ID backend trait and out of the core.

The alternative to `bluer` was extracting and generalising `improv-wifi`'s hand-rolled zbus layer so both protocols could share it. It would have been a refactor of known-good code rather than a rewrite against an unfamiliar API, but it keeps a BlueZ implementation as ours to maintain and means touching a published crate running on devices today. Taking `bluer` for bliti alone sidesteps that: `improv-wifi` is untouched, and bliti's experience becomes the evidence for whether it should follow.

## Choices weighed

### Handshake

Chosen: Noise `NNpsk0`, with the sticker secret as the pre-shared key.

Both ends bring only ephemeral keys and all authentication comes from the pre-shared secret, which matches a design where everything descends from the board and there is no other identity. `XXpsk0` would additionally give the device a long-term keypair, letting a client pin device identity independently of its sticker — which only earns its keep if a device's sticker secret can change while the device stays the same, and here it cannot.

Rejected:

- **MAC challenge-response.** Mutual and replay-resistant and trivially auditable, but no session key worth the name and no forward secrecy.
- **PAKE (SPAKE2+, CPace).** What Matter does for this flow, and it earns its complexity by making a shared secret unguessable from a transcript however small its space — which the memory-hard derivation already buys here, so it would pay twice for one property. Rust options are also thin. Worth revisiting only if the derivation is ever made cheap.
- **BLE link-layer pairing with out-of-band data from the QR.** Moves the problem into the Bluetooth stack: BlueZ out-of-band pairing is awkward, phone support is uneven, and browsers cannot drive pairing at all.

### Transport binding

Chosen: GATT now, L2CAP later, for three reasons that stack.

Browsers are GATT-only, and the web application is the first milestone's only client. L2CAP does not replace GATT in any case — a connection-oriented channel is identified by a PSM the client must first read from a characteristic, so GATT remains the way in. And nothing in the first milestone moves enough data to notice the difference.

Adding L2CAP later is cheap in a way the advertisement is not: a GATT service is discovered by UUID, so a characteristic publishing a PSM can appear whenever it is written, and clients that do not know about it ignore it. The end state is both, chosen per client, with an identical stack above.

### Streams, and why not QUIC

The property wanted is QUIC's — either end opens a stream without coordinating — and it is the part of the design expensive to change later, so it belongs in the first milestone. But QUIC itself does not fit:

- **The datagram floor.** QUIC requires a path carrying 1200-byte datagrams; the attribute protocol tops out well below that and negotiates lower in practice.
- **TLS does not detach.** QUIC's packet protection comes out of the TLS 1.3 key schedule, so a full TLS exchange would run after Noise has already authenticated both ends and produced a key.
- **Reliability over reliability.** BLE is already reliable and ordered, so QUIC's streams would not escape head-of-line blocking — the blocking happens underneath regardless — while paying for loss detection and congestion control that duplicate the link.
- **Browsers cannot send UDP.** Compiling a QUIC stack to wasm does not rescue the web application.

What delivers the property is a stream multiplexing layer inside the Noise channel, which is QUIC's stream layer with the transport machinery removed. `yamux` is exactly that. Failing it, hand-rolling is a few hundred lines, and QUIC's identifier convention is worth copying either way: low bits encoding which side opened the stream and whether it is unidirectional, so both ends allocate from disjoint spaces.

## Outstanding risks

**TPM seed stability is unverified, and it is the failure that would kill a model line's stickers at once.** The endorsement seed survives the ordinary clear operation, but a platform-authorised command exists to change it, and what a given firmware does on a BIOS-level TPM reset, on disabling and re-enabling a firmware TPM, or across a firmware update, has not been tested. A discrete TPM is far better placed — its seed is fixed at manufacture and attested by a vendor certificate, with no BIOS path reaching it — so the exposure is concentrated on machines with firmware TPMs. Test this on the hardware to be shipped before any sticker is printed for it.

**The derivation is killed rather than failing** when memory is short, so the pre-flight check or child-process isolation in `BLI-KEY` is required rather than defensive.

It runs on the first start after imaging and after a board change, not on every start, so that check sits off the ordinary path.

**BlueZ userspace is a deployment prerequisite** rather than something to assume: it was absent from the original device image and installed by hand; the reimaged prototype has it pre-installed.

**BlueZ must be configured peripheral-only, and this is a deployment prerequisite too.** `[GATT] Client = false` in `/etc/bluetooth/main.conf` is required for the channel to work at all: without it the device does reverse GATT discovery of the central, hits an encryption-gated attribute, tries to pair, is refused, and disconnects mid-session (see "The channel over BLE"). The packaged unit must ship this configuration.

## Advertising: settled, and verified on the air

BLI-ADV now carries the payload in the local name, and the advertisement is accepted by the
prototype's legacy-only controller. The shape is a 128-bit service UUID in the advertisement, so a
client platform that can only filter on that still can, and the handle, salt and version as 21
characters of unpadded base32 in the local name, which is the one element a host places in the scan
response.

Why the original design could not work: it specified which element goes in the advertisement and
which in the scan response, and BlueZ offers no way to say. Its advertisement interface carries
advertising data only — it reports `MaxScnRspLen` as a capability but exposes no scan response
content. On the prototype, flags, the service UUID and service data come to fifty-two bytes and
registration fails with `Invalid Parameters (0x0d)`. It appeared to work on an x86 machine only
because that controller does extended advertising and reports `MaxAdvLen 251`, so the split stopped
mattering. Both run bluetoothd 5.85.

The payload lives in `bliti-core` rather than in the daemon, because the web application parses the
same bytes and compiles the same crate to wasm.

Verified between the prototype and a laptop in the same room: the laptop holding the prototype's
sticker reports `MATCHES`, holding a different sticker reports `another bliti device` and then that
nothing matched, and the payload recovered from the human-readable rendering beneath the code matches
just as the URL does.

## The channel over BLE: working, repeatable, and end to end

The whole of the first milestone runs on the air, repeatably: the laptop matches the sticker,
connects, completes the `NNpsk0` handshake, receives the device's hostname and five addresses across
two interfaces in both families on a stream the device opens without being asked, sends a line of
text, and the device prints it to its journal. Nine consecutive connections in a row — some with the
laptop forgetting the device first, some reconnecting without — all completed both directions and the
device printed every line. Verified between the prototype and a laptop in the same room.

Three things stood between "reached once" and "repeatable", and all three are now fixed. Two were
daemon code, one was a device-side BlueZ configuration.

**The device tore down every connection with an authentication failure, killing the session before
the text was served.** This was the "connects but never resolves services" blocker, and a `btmon`
trace on both ends found the cause. On any connection, the device's `bluetoothd` acts as a GATT
client and resolves the *central's* GATT database in reverse. The laptop exposes an attribute whose
read requires encryption; the device, wanting to read it, sends an SMP Security Request to pair; the
laptop refuses (browsers and this client do not pair); the device disconnects with reason
Authentication Failure. Because the disconnect landed mid-discovery, the laptop never finished
resolving the device's services. This is not specific to the laptop: a phone central exposes
encryption-gated attributes too (iOS ANCS among them), so it would fire against the eventual web
client as well.

The fix is device-side and belongs to a peripheral-only appliance: `bluetoothd` is told not to be a
GATT client. In `/etc/bluetooth/main.conf`, `[GATT] Client = false` (and `ReverseServiceDiscovery =
false` alongside it) disables the reverse discovery, so the device never reads the central's
attributes, never escalates to pairing, and never disconnects. **This is a deployment prerequisite,
like BlueZ userspace itself** — it is not something the binary can set, and it must ship with the
packaged unit or the device image. A bliti device is peripheral-only, so disabling the GATT client
costs it nothing.

**The device stopped advertising after the first client and never resumed.** A legacy controller
stops advertising the instant a client connects and does not restart when it leaves; the
advertisement stays registered with BlueZ (it reports one active instance and no error), but nothing
goes out, so the next client cannot discover the device. The daemon now fires a signal when a session
ends and re-registers the advertisement in response, which puts the device back on the air. This was
most of the earlier "connections after the first" symptom: the device was simply not discoverable
after the first session, so a reconnect either found nothing or connected to stale state.

**Discovery was slow and unreliable on the first attempt after a device came on the air.** The
advertisement used BlueZ's default interval, over a second, so a scanning client's window caught it
only intermittently and the first attempt often heard nothing. The advertisement now asks for a
100–150 ms interval. With it, discovery is prompt and every one of the nine test connections found
the device.

Fixed earlier, and worth keeping: **the device could not tell when a client went away.** The session
read until its transport ended, and the transport only ended when the session dropped it, so the two
waited on each other and the device stayed busy with a client that had left. The daemon now watches
whether the client is still subscribed and ends the session when it is not.

**Left as a known rough edge:** the daemon handles `ctrl_c` (SIGINT) for a clean shutdown but not
SIGTERM, which is how systemd stops it — so `systemctl stop` kills it without unregistering its GATT
application, and BlueZ can retain a stale registration until the next `bluetoothd` restart. It did not
affect the working runs, but the packaged unit should either handle SIGTERM or accept that the GATT
state is cleared when `bluetoothd` restarts.

## The web application

Built as two halves, split where the protocol ends and the browser begins.

`crates/bliti-web` compiles to wasm and carries everything with protocol in it: reading a sticker,
recomputing and matching the advertised handle, the `NNpsk0` handshake, the stream layer, and the
JSON messages. It is the same `bliti-core` the daemon and the command-line client run, which is the
whole point of the split — one implementation of the key schedule rather than a Rust one and a
JavaScript one that must agree forever. It takes the core without default features, so argon2 is not
in the build at all: a client reads the sticker secret from the payload and only computes the handle.

`www/` is the page: Web Bluetooth, the camera, and the interface, in plain JavaScript. There is no
bundler and no npm. `wasm-bindgen --target web` emits an ES module the page imports directly, and QR
capture uses the browser's own `BarcodeDetector` rather than a scanning library, which is available
on Chrome for Android — the same platform Web Bluetooth needs — and is why no dependency is required
for it. Where it is absent the camera button does not appear and the rendering can be typed instead.

### What is verified, and how

Without a browser, by running the wasm through node against the prototype's live advertisements:

- The wasm computes the same handle the device does. Three real advertisements captured from the
  prototype under three different salts all match the sticker, which is BLI-ADV's rotation property
  demonstrated on real data rather than on a fixture. A payload with a handle that is not this
  device's does not match, a name that is not a payload at all is passed over, and a payload carrying
  a version marker this client does not hold is read as exactly that rather than as a non-match.
- Both sticker-reading paths give the same secret: the URL a code encodes, and the human-readable
  rendering typed in.
- The handshake runs in wasm. Opening a channel emits a 48-byte `NNpsk0` first message, chunked to
  twenty bytes a write and handed to the JavaScript write function, and two channels emit different
  ephemeral keys. That is the wasm-specific risk cleared: `getrandom`'s browser backend reaches
  `snow`, and the framing and the transport pump work.

In a browser, the page loads, instantiates the wasm, and runs through: headless Chrome reaches the
Web Bluetooth check and reports its absence, which is the correct answer there.

What is left is the part that needs a real adapter: the chooser, GATT, notifications, and the channel
against the device. That is a browser test on hardware, not something the test harness can reach.

### The chooser cannot be narrowed, and BLI-WEB says it can

BLI-WEB says that where the browser offers a chooser rather than the advertisements themselves, the
application "filters that chooser by the local name, so that the device whose sticker was read is the
one presented". No browser can do this. Web Bluetooth filters on an exact name or a name prefix, and
the local name is the handle followed by the salt, both of which change every fifteen minutes and
neither of which a client can predict before hearing the advertisement it is trying to filter for.
Reading the advertisements directly would solve it, but `requestLEScan` is behind a flag and is not
something an operator can be asked to turn on.

What the application does instead is filter the chooser to devices carrying the bliti service UUID,
and check the device the operator picks against the sticker before anything is sent to it: not this
device's payload, a version this client does not hold, or a different bliti device are each reported
as what they are. The spec needs changing to describe that, and it is the one place where the
implementation knowingly departs from a spec.

### Building it

`crates/bliti-web/build.sh` builds the wasm and its bindings into `www/pkg`, which is generated and
not committed. It needs the `wasm-bindgen` CLI at the same version as the crate, and the wasm target
installed. The `.cargo/config.toml` added at the workspace root passes `getrandom`'s backend cfg for
the wasm target only, which `snow` needs and which the feature alone does not supply. Serving `www/`
is covered under Development affordances.

## The prototype's identity, to check against after it is reimaged

Reimaging the prototype is a direct test of the claim the whole design rests on: that a board's
sticker is reproducible from the board alone, with no per-device record kept. Nothing needs saving
off the device — the cache is a cache — and the values it should come back with are these, recorded
before the image was replaced.

| | |
| --- | --- |
| winning source | Raspberry Pi serial |
| device-tree serial | `f3756510f632cfad` |
| customer OTP | 32 bytes of zeros, so precedence falls through it |
| sticker secret | `cb89bf939b867ec6e15530a6b92db98a14f170e1f4c9ff218cbd460e2140ccbd` |
| sticker URL | `https://bliti.tamanu.app/#AcuJv5Obhn7G4VUwprktuYoU8XDh9Mn_IYy9Rg4hQMy9` |
| printed rendering | `AHFY-TP4T-TODH-5RXB-KUYK-NOJN-XGFB-J4LQ-4H2M-T7ZB-RS6U-MDRB-IDGL-2` |
| adapter address | `88:A2:9E:CB:28:E1` |

The secret is the same value the production known-answer test pins, because that vector is taken from
this board's real serial. A reimaged board that derives anything else means the chain is not
reproducible, and every sticker already printed is in question.

Two things the reimage takes with it, both already known: ~~BlueZ userspace was installed by hand and
will need installing again~~, and the binary is deployed to `/tmp`, which does not survive. The image
it ran on was Ubuntu 26.04 LTS, kernel 7.0.0-1015-raspi, with bluez 5.85.

Correction: the new image has bluez pre-installed.

**Reimage confirmed the reproduction.** After the board was reimaged (Ubuntu 26.04 LTS, kernel
7.0.0-1017-raspi, bluez 5.85), a build with the `tpm` feature off — correct for a board with no TPM,
and the only way it cross-compiles without a target sysroot for `tss2-sys` — derived the sticker
fresh with nothing carried over. It came back with the recorded values byte for byte: RPi serial
winning over placeholder OTP, URL `…#AcuJv5Obhn7G4VUwprktuYoU8XDh9Mn_IYy9Rg4hQMy9`, rendering
`AHFY-TP4T-…-IDGL-2`. The chain is reproducible from the board alone; the design's central claim holds
across a full reimage.

## Milestones

1. **A channel.** Board ID reading, both derivations, sticker generation, advertising a rotating handle, the `NNpsk0` handshake, and a stream layer over GATT carrying JSON, plus the web application driving all of it. Two things ride on it: a line of text from the browser that the device prints, proving the client-to-device direction, and the device's hostname and addresses, proving the other. Everything genuinely novel is here; what follows is operations on a pipe that already works.
2. **Wi-Fi.** Joining a network, and putting the device into access-point mode — the case Improv cannot express and the reason this protocol carries Wi-Fi at all. The `WifiConfigurator` trait and NetworkManager backend in `improv-wifi` are a starting point, though the access-point case reaches past what that trait expresses.
3. **The rest of provisioning.** Device description — model, board ID, OS image, software versions, network name, local time, battery, temperature. Physical identification, by blinking an LED or drawing on a display where one is fitted. Hostname and timezone. Enrolment with a server. Recent logs or health output. Reboot.
4. **A native application.** Android or iOS, once the protocol has stopped moving.

## Deferred, and free to defer

Advertising only in a window, and waking a quiet device over the air, are carried as draft cards in the card breakdown.

**Multiple concurrent versions.** The current device controller supports one advertising set, so a device advertises one version at a time and would have to alternate to support several, at a cost in discovery latency. Nothing needs building now; reserving the version marker in both payloads is what keeps it available, and that is in the first milestone because adding it later breaks every deployed device.

## Build order

Implementation began with the protocol core crate (`crates/bliti-core`): the derivation chain,
the sticker payload, and the channel. It is pure, unit-tests without hardware or BlueZ, and is the
layer the daemon and the web application both build on. The concrete TPM and OTP board-ID backends
are held back until their byte encoding is verified on the hardware to be shipped, because that
encoding is versioned into every sticker (see Outstanding risks); the framework selects whichever
backends are registered, so they slot in without disturbing the schedule.

- [x] Board ID trait, the test backend, and the Raspberry Pi serial and SMBIOS backends (RPi read verified against the prototype's device-tree serial)
- [x] TPM Endorsement Key and one-time-programmable backends, with their byte encodings verified on hardware (see Measured on hardware)
- [x] Source precedence, sentinel rejection, and the failure when nothing is usable
- [x] Key schedule: both derivations, the version marker, and known-answer tests pinning them (handle and sticker-secret wiring always-on; the full 2 GiB production vector pinned as an ignored test)
- [x] Sticker cache logic: the cached board ID, platform serial and source kind, and the cheap comparison against the board at start (`board_id::evaluate_cache`), plus the pre-flight memory check (`key_schedule::check_memory`). On-disk persistence of the cache is daemon work
- [x] Sticker payload encoding (fragment and human-readable rendering) and the QR URL. Rendering the QR *image* is daemon work
- [x] Noise `NNpsk0` handshake, tested with wrong-secret, replay, and tamper cases
- [x] Framing and reassembly
- [x] Stream layer over the framed transport: the `NoiseStream` encrypt/frame adapter (any `AsyncRead + AsyncWrite`), yamux multiplexing with a runtime-agnostic driver, and the handshake-over-transport helpers. Tested end to end over an in-memory duplex: bidirectional exchange, streams opened from each end, and one stream closing while others stay alive
- [x] JSON message types, including the unknown-message reply that keeps the channel open
- [x] GATT server and characteristics via `bluer`, with a byte-stream adapter over the write and notify characteristics
- [x] Advertisement and scan response construction within the 31-byte budgets, with salt rotation — the budget arithmetic is implemented and tested, but BlueZ will not pack it as specified on a legacy controller (see Blocked, above)
- [x] Daemon tying it together, with identity establishment, the cache on disk, and sticker generation. Run on the prototype under `systemd-run`, so both streams reach the journal; a packaged unit is still to write
- [x] Address and hostname reporting, including unsolicited sending on change
- [x] Text echo to standard output (verified over the air: the device prints the client's line to its journal)
- [x] Advertising resumes after a session ends, and a short advertising interval so discovery is prompt — both verified across nine consecutive connections
- [x] Web application: the wasm client crate and the page — sticker reading by link, typing, and
      camera, matching, handshake, streams, and both directions. Verified as far as a browser-free
      harness reaches (see The web application); the channel against a device is still to test on
      hardware
- [x] A `scan` subcommand doing the client half of discovery and matching from the command line, so discovery can be exercised without a browser
- [ ] A packaged systemd unit — must ship the peripheral-only BlueZ config (`[GATT] Client = false`) and should handle SIGTERM for a clean GATT unregister

The core also compiles for `wasm32-unknown-unknown` with `--no-default-features` (verified), which is
what lets the web application share the key schedule and handshake; a wasm consumer enables
`getrandom`'s wasm backend, as any wasm crate depending on `snow` does. `snow` is taken with
`default-features = false`, because its `std` feature pulls in `ring`, which is unused here and does
not belong in a wasm build.

## Measured on hardware

Taken on the Raspberry Pi 5 prototype and on an x86-64 UEFI machine with an Intel firmware TPM.

**The Endorsement Key name is the key's name as the TPM computes it**: a two-byte algorithm
identifier followed by the digest of the public area, 34 bytes under SHA-256. The backend's output
is byte-identical to what `tpm2_createek` and `tpm2_readpublic` produce, and regenerating the key
from the seed under the pinned template gives the same name every time. The test takes the expected
name from the environment, so it pins against an independent implementation without baking one
machine's Endorsement Key into the repository.

**The derivation is reproducible across architectures and across how it is computed.** One
known-answer vector holds on x86-64 and aarch64, with the argon2 lanes computed concurrently and in
sequence. That is what lets a sticker generated on a desktop match the device that derives its own
secret. On the Pi 5 the derivation costs **2.2 s with the lanes concurrent and 3.1 s in sequence**,
so `parallel` is on by default; it changes only the speed.

**Precedence behaves as specified on both boards**: the UEFI machine selects its TPM over its SMBIOS
system UUID, and the Pi selects its device-tree serial after falling through unwritten
one-time-programmable memory, which reads as 32 bytes of zeros on the prototype.

Two things worth carrying forward. The prototype exposes its serial at
`/proc/device-tree/serial-number` and its customer OTP at `/sys/bus/nvmem/devices/nvmem_cust0/nvmem`,
both root-readable only. And the UEFI machine's system UUID begins `4c4c4544`, which is ASCII
"LLED" — a vendor service tag reformatted as a UUID, exactly the weak case BLI-KEY describes, met on
the first UEFI machine tried. It is evidence for measuring the entropy of that source before relying
on it, which the test cases already carry.

**Which backends a build registers decides which source wins**, and so which secret a board derives.
A build of the daemon or the generator without the `tpm` feature would derive a Pi-style serial
secret on a machine whose sticker was printed from its TPM. Both binaries therefore depend on that
feature unconditionally rather than leaving it to a build flag.

## Development affordances

These serve the first phase and are deliberately absent from the specs, because they do not outlive it.

The web application is served from localhost during development, with `python3 -m http.server 8749 --bind 127.0.0.1` in its `www/` directory or anything equivalent. That is a secure context, so the camera and Bluetooth are available without a certificate. A plain-HTTP origin on a LAN address is not a secure context and will not do, so a phone testing against a development machine reaches it by port-forwarding to its own localhost rather than over the network.

A sticker is not needed to exercise the application. The payload can be pasted from the human-readable rendering, and the camera path tested against a code on screen.

Opening the link from a real code is verified against the deployed origin as a post-deploy step, being the one path that needs the production host live.
