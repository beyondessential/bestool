---
status: draft
---

# improv-device: QR-anchored BLE device provisioning

A companion to Improv-Wi-Fi, but not an Improv protocol: a headless device advertises an opaque handle over BLE, and a phone that has scanned the QR sticker on the device's enclosure — and only such a phone — can recognise it, authenticate to it, and open a two-way channel for provisioning.

The sticker is the second factor. It replaces the button press or on-screen code that Improv-Wi-Fi and similar protocols use to prove physical presence, because the devices we are targeting have neither a usable button nor a screen.

## Overview

The chain, as sketched in the kickoff conversation:

1. Read a **board ID** from firmware — the Raspberry Pi board serial, or the SMBIOS system UUID on UEFI machines.
2. Derive a **sticker secret** from it under a fixed, public constant. This is what goes in the QR code printed on the sticker stuck to the outside of the enclosure.
3. Derive an **advertised handle** from the sticker secret under a second fixed, public constant. This is what the device broadcasts over BLE.
4. A phone scans the sticker, recomputes the handle, and looks for that handle in BLE advertisements. Only a scanner that has seen the sticker can tell which advertisement belongs to which box.
5. Phone and device run a **challenge-response** so that neither a replayed advertisement nor a passive listener gets a session, and so the phone knows it is talking to the device whose sticker it scanned.
6. On top of that authenticated session sits a **two-way RPC** for provisioning: Wi-Fi, enrolment, reconfiguration, diagnostics.

### Threat model

Everything is derived from fixed public constants and the board ID. There is no fleet key and no per-device state: the chain is reproducible from the board alone, at manufacture or any time after. This is a deliberate choice, and it sets what the scheme can and cannot buy.

In scope:

- **Telling N devices in one room apart.** An operator holding a sticker can pick that device out of everything advertising nearby.
- **Light protection against interception.** A passive listener on the BLE link learns neither the sticker secret nor the session contents, and cannot replay an advertisement into a session.
- **One-wayness of the sticker.** Reading the QR must not yield the board ID, and that must hold even for someone who knows the derivation constant. The board ID therefore never appears in the QR payload, in the advertisement, or anywhere else a passer-by can reach.

Out of scope, explicitly:

- **An attacker who has had access to the device, or who knows its board ID.** Such a party can derive the sticker secret and impersonate or connect to the device. This is accepted, not defended against.
- **An attacker holding a photo of the sticker.** Same position: the sticker is the credential.

Sitting between the two, and worth naming because it is neither: because the constants are public, discovering a device needs neither the sticker nor physical access — only a board ID, and board IDs can be searched rather than known. See "Cost of deriving the sticker secret".

## Behaviour

### Board ID

The device reads a stable, firmware-provided identifier for the board it is running on.

- **Raspberry Pi**: the device-tree serial number (`/proc/device-tree/serial-number`, mirrored in `/proc/cpuinfo` as `Serial`). 64-bit, readable by any process.
- **UEFI/SMBIOS**: the SMBIOS System UUID (`/sys/class/dmi/id/product_uuid`), 128-bit. Root-only on Linux, which is fine for a daemon that already needs root for NetworkManager and BlueZ. Board serial and product serial are siblings worth considering as fallbacks.
- **Neither present**: containers and some VMs expose no DMI and no device tree — the environment this doc was drafted in has neither. The behaviour when no board ID can be read has to be defined rather than left to a panic.

The board ID is **not a secret**. Any software on the box can read it and it is printed on shipping manifests. Nothing in the scheme may depend on it staying hidden — only on it being expensive to *search for*, which is a different property and is handled at the derivation step.

Its job is to make the sticker secret *reproducible*: the same sticker can be regenerated from the board alone — reprinted after damage, or generated in bulk from a manifest of board IDs — with no per-device database and nothing to keep in sync.

How much entropy each source actually carries is a question to settle against the boards we ship rather than assume. The SMBIOS UUID is 128 bits but some vendors ship a constant, a zeroed, or a MAC-derived value. Raspberry Pi serials are 64-bit on Pi 4 and 5, but on earlier boards the high half is zero and the value is effectively a 32-bit OTP word.

### Sticker secret

The value carried in the QR code, and the only secret in the system. Derived from the board ID under a fixed constant.

The constant is public — it can be compiled into the device, the sticker generator, and the phone app without weakening anything, because a hash does not run backwards. What the constant buys is domain separation, not secrecy.

The board ID does **not** go in the QR alongside it. Putting it there would hand the board ID to anyone who photographs a sticker, which is precisely the property the derivation exists to provide.

Because the secret is per-board and derived, compromising one device yields that device and no other. There is no fleet-wide value to leak.

### Cost of deriving the sticker secret

With a public constant, the only thing stopping someone enumerating the board-ID space offline — computing every sticker secret, every handle, and matching them against advertisements they can hear — is the cost of the derivation. That attack needs neither the sticker nor access to any device, so it is not covered by what the threat model puts out of scope.

A plain hash makes it cheap: 32 bits of board ID against a fast hash is minutes of GPU time, and 64 bits is not a comfortable margin either.

Making the first derivation **memory-hard** — argon2id or scrypt rather than a plain hash — closes this without changing the model. The constant stays public and the derivation stays reproducible from the board alone; only the cost of doing so moves. Stickers are generated once at manufacture, so the generator can afford whatever we ask of it.

Parameters should be pushed hard, to seconds of work rather than milliseconds. The useful mental model is that an attacker's cost per guess scales with memory times iterations, and a GPU's parallelism is capped by its VRAM divided by the memory parameter — so memory buys more than time does, since it bounds how many guesses can run at once rather than just how long each takes.

**The ceiling is the weakest device, not the generator.** The device needs the secret at runtime to compute its handle and run the handshake, so whatever we choose has to run there. Time is merely slow on a small board; memory is a hard wall, and a parameter larger than the board's RAM cannot run on it at all, ever. Two ways to buy headroom:

- **Cache the derived secret on the device** after first boot. The cost is paid once per imaging rather than every boot, which makes a long derivation tolerable. Memory is still capped by the board's RAM. This keeps the device self-sufficient: the cache is an optimisation, not a source of truth, and rederiving from the board is always available. A cache that does not match the board it is on — an SD card moved between boards — is detectable and simply rederives.
- **Derive at imaging time on good hardware** and install the result, with the device never deriving at all. This lifts the memory cap entirely. The model is unchanged, because there is still no secret key and the value is still reproducible from the board ID and the public constant by anyone with the compute — but the device can no longer recover its own secret unaided, only with a machine that can run the derivation. Recovery stays possible off-device, since the board ID is public and readable from the board.

The second derivation, sticker secret to advertised handle, stays a fast hash either way. The phone compares against every advertisement it hears while scanning, and a memory-hard function in that loop would be felt.

### Advertised handle

A one-way hash of the sticker secret, truncated to a handful of bytes, broadcast in the BLE advertisement.

- The derivation constant here need not be secret. Hashing is one-way, so a listener who hears the handle still cannot recover the sticker secret; a phone app can carry the constant in the clear.
- The handle must be long enough that collisions across a site are implausible, and short enough to leave room in a 31-byte legacy advertisement. Eight bytes is a comfortable default.
- Stickers outlive software. Once a box is in the field its sticker is fixed, so a later change to the constants or the derivation parameters must not orphan it: the payload carries a version, and a device has to be able to derive and advertise under every version it still supports — which means the advertisement carries the version too, or the device advertises one handle per supported version.
- A **static** handle makes the device passively trackable — a fixed beacon following the box around. If that matters, the handle can be rotated without a clock: advertise a short random rotation salt in the clear alongside `H(sticker secret, salt)`, and roll the salt periodically. A scanner recomputes for the observed salt; a passive observer cannot link two advertisements from the same device. This also means the BLE adapter must use resolvable private addresses, or the MAC becomes the tracker regardless.

### Discovery and matching

The phone scans, computes the expected handle from the scanned sticker, and matches.

Two platform constraints shape what we can put where:

- **iOS CoreBluetooth** can only filter a scan by service UUID; it cannot filter by service data. The app scans for our service UUID and matches the handle client-side. It never sees the peripheral's MAC, which is fine because we identify by handle.
- **Web Bluetooth** (the browser test page) has no passive scanning without an experimental flag, and the user must pick the device from a browser-drawn chooser. Filtering that chooser by service data is not reliably supported; manufacturer data and name prefix are. Putting a rendering of the handle in the advertised local name — so the chooser shows something the tester can match by eye and `namePrefix` can filter on — is the pragmatic answer, and costs nothing since the handle is not secret. Worth advertising both: service data as the canonical carrier, local name for display and web filtering.

### Authentication

The phone must prove it holds the sticker secret; the device must prove the same, so that a replayed or spoofed advertisement does not yield a session. Both directions matter, and the session that follows should be encrypted.

The sticker secret is a full-width value, not a six-digit code a human types, which is the fact that decides the mechanism — see "Implementation options". Its resistance to offline guessing comes from the memory-hard derivation, not from the board ID's own entropy, and that matters here as much as at the sticker: an eavesdropper who records a handshake can otherwise search the board-ID space against the transcript. The same defence covers both.

A separate value in the QR for the challenge-response is not needed. The handshake already proves possession of the sticker secret in both directions, which is what "this is the box whose sticker I scanned" and "you scanned my sticker" both reduce to. A replayed advertisement buys an attacker nothing, because they cannot complete the handshake behind it.

Verifying the device's board ID directly is not available: it cannot go in the QR, and the derivation does not run backwards. Possession of the sticker secret is the proof, and it is equivalent — deriving it requires the board ID.

### Session and provisioning

Once authenticated, a two-way, ordered, reliable message channel carries a small request/response RPC. Candidate operations, roughly in the order we would want them:

- Describe the device: model, board ID, OS image, software versions, current network state.
- Physically identify: blink an LED, or draw on the LCD where one is fitted, so the operator can confirm which box they are talking to.
- Configure Wi-Fi. The `WifiConfigurator` trait and NetworkManager backend in `improv-wifi` are directly reusable here.
- Set hostname, timezone.
- Enrol the device with its server.
- Fetch recent logs or health output for field diagnosis.
- Reboot.

### Web test page

A static page using Web Bluetooth that does the scan-match-authenticate-RPC flow, so the protocol can be exercised before any native app exists. Chrome on Android and desktop only; Safari and Firefox have no Web Bluetooth, and on iOS it requires a third-party browser shell. Good enough for development, not a shipping story.

## Implementation options

### Choice of derivation function

Settled in shape — memory-hard for the first step, pushed to seconds of work, fast for the second — but not in detail.

- **argon2id** is the current default recommendation and has a maintained pure-Rust implementation. Parameters are three-dimensional: memory, iterations, parallelism.
- **scrypt** is older and simpler to parameterise, with a worse memory-hardness margin.
- For the fast second step, BLAKE3 keyed mode gives domain separation and a truncatable output in one primitive.

Parameters cannot be chosen until two things are measured: the weakest board's available RAM, which is a hard cap on the memory parameter, and how long a candidate setting actually takes there. Both belong in the first milestone, before anything is printed on a sticker — the constants and parameters are baked into every sticker in the field the moment one ships.

### Handshake

- **MAC challenge-response.** Each side sends a nonce; each replies with a MAC over both nonces under the sticker secret, domain-separated by direction. Mutual, replay-resistant, trivially implementable and auditable. No session key worth the name and no forward secrecy — anyone who later learns the sticker secret can decrypt a recorded session.
Decided: **Noise with a pre-shared key**. The rest is kept as the record of what was weighed.

- **Noise with a pre-shared key** (`NNpsk0` or `XXpsk0`, via `snow`). The sticker secret is the PSK. Mutual authentication from the PSK, a fresh session key and forward secrecy from the ephemeral DH, an encrypted transport falls out of it, and the pattern is well-analysed. Adding a device static key later lets a phone pin device identity across a sticker reprint.
- **PAKE (SPAKE2+ / CPace).** What Matter does for exactly this flow. A PAKE earns its complexity by making the shared secret unguessable from a transcript no matter how small its space is, which is what lets Matter use a six-digit PIN. Here the memory-hard derivation already buys that, so the PAKE would be paying twice for one property — and Rust SPAKE2+ options are thin. Worth revisiting only if the derivation ends up cheap.
- **BLE link-layer pairing with OOB data from the QR.** Moves the problem into the Bluetooth stack. BlueZ OOB pairing is awkward, phone support is uneven, and Web Bluetooth cannot drive pairing at all. Treating BLE as a dumb pipe and doing crypto at the application layer keeps every client viable.

Noise-with-PSK looks like the right weight for this.

### QR payload shape

- **An `https://` URL with the payload in the fragment.** A generic phone camera opens the web page; the fragment never reaches a server, so the secret stays local; a native app can claim the link so scanning opens the app when installed. Needs a short host to keep the QR small.
- **A custom URI scheme.** Clean, but a stock camera app does nothing useful with it.
- **Bare text.** Smallest QR, app-only.

Either way the payload wants a version marker so the scheme can change, and a human-readable rendering printed beneath the QR as a fallback when the code is scuffed.

Prior art for the payload and the flow generally: Matter's commissioning (QR with a setup payload, a short discriminator advertised over BLE for matching, PASE over the BLE link), HomeKit setup codes, and Improv-Wi-Fi itself.

### Transport binding

- **GATT characteristics**, as Improv-Wi-Fi does: a write characteristic for commands and a notify characteristic for results, with framing and reassembly over the negotiated ATT MTU. Works on every client including Web Bluetooth. Modest throughput.
- **L2CAP connection-oriented channels.** A real stream with far better throughput; supported by BlueZ, iOS 11+, and Android API 29+, but not by Web Bluetooth.

Defining the protocol over an abstract message transport and shipping the GATT binding first keeps the web page working and leaves L2CAP available later for anything bulky.

QUIC over BLE was raised as a possibility. It wants a datagram transport we would have to synthesise, and it brings a TLS handshake we do not need once Noise is in play — the cost is not obviously repaid.

### Crate layout

Working name for the protocol and its crates: **improv-device**. It lives in this repository but stands alone — its own binary, not a `bestool` subcommand, and nothing in the `bestool` binary depends on it. That keeps the option of moving it to its own repository later without unpicking anything.

Three crates:

- **The BlueZ peripheral**, extracted from `improv-wifi`. That crate already carries a working advertisement, GATT, and application layer over zbus, currently private to it; generalising it so both protocols can register their own services and advertisements is the same work either way, and doing it once avoids two copies drifting. Needs a name — nothing obvious is free on crates.io.
- **The protocol core**: board ID reading, the key schedule, the handshake, framing, and RPC types. No BlueZ and no hardware, so it unit-tests anywhere.
- **The daemon**: the binary that ties the core to the peripheral, plus sticker generation.

Board ID reading sits inside the core as backends behind a trait — Raspberry Pi, SMBIOS, and a test backend — rather than its own crate. It can move out if something else needs it.

The core earns its separation from the daemon for a reason beyond tidiness: the web test page needs the same derivations and the same handshake in the browser. Compiling the core to wasm keeps one implementation of the key schedule rather than a Rust one and a JavaScript one that must agree forever. This constrains the core to stay free of I/O and of anything that will not build for `wasm32-unknown-unknown` — `snow`'s pure-Rust resolver does.

### Sequencing the extraction

`improv-wifi` is published and running on devices today, so pulling its BlueZ layer out is a refactor of live code rather than greenfield work. Doing it as its own change first — behaviour-preserving, no new protocol in the picture, existing tests as the guard — keeps a regression there from being tangled up with a new protocol's bugs.

## Open questions

- [ ] Card identifier for this work, so the working doc, plan, and specs land in the right place. None yet; the doc sits under a provisional directory until there is one.
- [ ] Which board is the weakest in scope, and how much RAM does it have? This caps the derivation's memory parameter.
- [ ] Does the device derive its own secret (cached after first boot, memory capped by that board), or is it derived on good hardware at imaging time and installed (no cap, device cannot self-recover unaided)?
- [ ] argon2id or scrypt, at what parameters?
- [ ] **improv-device** is the working name, which is fine while it stays here. Before anything is published it wants a second look: Improv Wi-Fi is someone else's protocol, and a crate called `improv-device` sitting next to our `improv-wifi` — which really is an implementation of that spec — would read as another one. This protocol has nothing to do with it.
- [ ] A name for the extracted BlueZ peripheral crate. `bluez-peripheral` is taken on crates.io.
- [ ] Which Noise pattern: `NNpsk0` is enough for the sticker secret alone; `XXpsk0` adds a device static key, which would let a phone pin device identity across a sticker reprint. Does that matter?
- [ ] Is passive-tracking resistance (rotating handle, private addresses) in scope for the first version?
- [ ] Which provisioning operations are in the first milestone, and is Wi-Fi configuration one of them or does Improv-Wi-Fi keep that job?
- [ ] RPC payload encoding — framing with winnow per house style, but the message bodies could be postcard, CBOR, or hand-rolled.

## Testing notes

- The SMBIOS path is the testable one: a UEFI VM or CI runner has a system UUID where a Pi does not, so the whole derivation chain can be exercised without hardware.
- A board ID override for tests, and a defined behaviour when no board ID source exists at all.
- Measure the actual entropy of both board ID sources on hardware we ship: how many bits a Pi serial really carries on each model, and whether the SMBIOS UUIDs we see in the field are distinct rather than a vendor constant. The derivation parameters follow from the answer.
- Time the derivation on the slowest board in scope, since it sits on the boot path.
- Known-answer tests pinning both derivations, so a change to constants or parameters cannot silently invalidate every sticker already printed.
- Full handshake and RPC exercised over an in-memory duplex transport, with no BLE involved.
- Negative cases worth pinning down: wrong sticker secret, replayed advertisement, replayed handshake, truncated frames, a peer that authenticates and then sends garbage.
- Handle collision between two devices in range.
- BlueZ-level testing against a virtual controller is possible but heavy; the protocol core should not need it.
