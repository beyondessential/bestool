---
status: draft
---

# QR-anchored BLE device provisioning

A companion to Improv-Wi-Fi: a headless device advertises an opaque handle over BLE, and a phone that has scanned the QR sticker on the device's enclosure — and only such a phone — can recognise it, authenticate to it, and open a two-way channel for provisioning.

The sticker is the second factor. It replaces the button press or on-screen code that Improv-Wi-Fi and similar protocols use to prove physical presence, because the devices we are targeting have neither a usable button nor a screen.

## Overview

The chain, as sketched in the kickoff conversation:

1. Read a **board ID** from firmware — the Raspberry Pi board serial, or the SMBIOS system UUID on UEFI machines.
2. Derive a **sticker secret** from it with a keyed hash. This is what goes in the QR code printed on the sticker stuck to the outside of the enclosure.
3. Derive an **advertised handle** from the sticker secret with a second hash. This is what the device broadcasts over BLE.
4. A phone scans the sticker, recomputes the handle, and looks for that handle in BLE advertisements. Only a scanner that has seen the sticker can tell which advertisement belongs to which box.
5. Phone and device run a **challenge-response** so that neither a replayed advertisement nor a passive listener gets a session, and so the phone knows it is talking to the device whose sticker it scanned.
6. On top of that authenticated session sits a **two-way RPC** for provisioning: Wi-Fi, enrolment, reconfiguration, diagnostics.

Threat model, stated plainly: this is presence-plus-possession, not strong security. Anyone who has had physical access to the device, or a photo of the sticker, can impersonate or connect to it. What it must buy us is that a party with neither cannot discover, identify, track, or connect to the device.

## Behaviour

### Board ID

The device reads a stable, firmware-provided identifier for the board it is running on.

- **Raspberry Pi**: the device-tree serial number (`/proc/device-tree/serial-number`, mirrored in `/proc/cpuinfo` as `Serial`). 64-bit, readable by any process.
- **UEFI/SMBIOS**: the SMBIOS System UUID (`/sys/class/dmi/id/product_uuid`), 128-bit. Root-only on Linux, which is fine for a daemon that already needs root for NetworkManager and BlueZ. Board serial and product serial are siblings worth considering as fallbacks.
- **Neither present**: containers and some VMs expose no DMI and no device tree — the environment this doc was drafted in has neither. The behaviour when no board ID can be read has to be defined rather than left to a panic.

The board ID is **not a secret**. Any software on the box can read it, it is printed on shipping manifests, and Pi serials are not uniformly distributed. Nothing in the scheme may depend on it being unguessable.

Its job is to make the sticker secret *reproducible*: given the board ID and the fleet key, the same sticker can be regenerated — reprinted after damage, or generated in bulk from a manifest — with no per-device database.

### Sticker secret

The value carried in the QR code. Derived as a keyed hash of the board ID under a **fleet key**.

Because the board ID is public, the fleet key is the only thing standing between an attacker and every sticker secret in the fleet. Where that key lives is the single most consequential decision in the design — see the first open question.

### Advertised handle

A one-way hash of the sticker secret, truncated to a handful of bytes, broadcast in the BLE advertisement.

- The derivation constant here need not be secret. Hashing is one-way, so a listener who hears the handle still cannot recover the sticker secret; a phone app can carry the constant in the clear.
- The handle must be long enough that collisions across a site are implausible, and short enough to leave room in a 31-byte legacy advertisement. Eight bytes is a comfortable default.
- A **static** handle makes the device passively trackable — a fixed beacon following the box around. If that matters, the handle can be rotated without a clock: advertise a short random rotation salt in the clear alongside `H(sticker secret, salt)`, and roll the salt periodically. A scanner recomputes for the observed salt; a passive observer cannot link two advertisements from the same device. This also means the BLE adapter must use resolvable private addresses, or the MAC becomes the tracker regardless.

### Discovery and matching

The phone scans, computes the expected handle from the scanned sticker, and matches.

Two platform constraints shape what we can put where:

- **iOS CoreBluetooth** can only filter a scan by service UUID; it cannot filter by service data. The app scans for our service UUID and matches the handle client-side. It never sees the peripheral's MAC, which is fine because we identify by handle.
- **Web Bluetooth** (the browser test page) has no passive scanning without an experimental flag, and the user must pick the device from a browser-drawn chooser. Filtering that chooser by service data is not reliably supported; manufacturer data and name prefix are. Putting a rendering of the handle in the advertised local name — so the chooser shows something the tester can match by eye and `namePrefix` can filter on — is the pragmatic answer, and costs nothing since the handle is not secret. Worth advertising both: service data as the canonical carrier, local name for display and web filtering.

### Authentication

The phone must prove it holds the sticker secret; the device must prove the same, so that a replayed or spoofed advertisement does not yield a session. Both directions matter, and the session that follows should be encrypted.

The sticker secret is machine-generated and high-entropy, which is the fact that decides the mechanism — see "Implementation options".

Verifying the device's *board ID* against a value also carried in the QR is worth doing, but it is an operational integrity check — it catches a sticker on the wrong box — not a security control, since the board ID is not secret.

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

### Where the fleet key lives

- **Device derives at runtime.** The device holds the fleet key and computes its own sticker secret from its board ID. Simplest operationally, but one extracted device yields the key, and the key plus a list of board IDs yields the entire fleet. Board IDs are easy to come by.
- **Derived off-device, installed at imaging time.** Sticker secrets are computed wherever stickers are printed; the device is given only its own secret, as part of the image or first-boot provisioning. The device never holds the fleet key, so compromising a device compromises exactly that device. Reprinting still works from the board ID and the fleet key.
- **Per-device random, no fleet key.** Generate a random secret at imaging time, store it on the device, record it wherever the sticker is printed. Removes the fleet key entirely, and removes any dependence on board ID entropy — at the cost of needing a record to reprint a sticker.

The second option looks strongest, and it does not preclude the third: the device's behaviour is identical either way, since it only ever reads a secret it was given.

### Handshake

- **MAC challenge-response.** Each side sends a nonce; each replies with a MAC over both nonces under the sticker secret, domain-separated by direction. Mutual, replay-resistant, trivially implementable and auditable. No session key worth the name and no forward secrecy — anyone who later learns the sticker secret can decrypt a recorded session.
- **Noise with a pre-shared key** (`NNpsk0` or `XXpsk0`, via `snow`). The sticker secret is the PSK. Mutual authentication from the PSK, a fresh session key and forward secrecy from the ephemeral DH, an encrypted transport falls out of it, and the pattern is well-analysed. Adding a device static key later lets a phone pin device identity across a sticker reprint.
- **PAKE (SPAKE2+ / CPace).** What Matter does for exactly this flow. PAKEs earn their complexity when the shared secret is a six-digit PIN a human types. Ours is a machine-generated high-entropy value in a QR code, so the offline-guessing resistance a PAKE buys is not needed, and Rust SPAKE2+ options are thin.
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

- A transport-agnostic protocol core: payload encoding, key schedule, handshake, framing, RPC types. No BlueZ, no hardware, so it unit-tests on any machine and could go `no_std` later.
- Board ID reading: Raspberry Pi and SMBIOS backends behind a trait, plus a test backend, so the whole flow runs on a developer laptop and in CI.
- The BlueZ peripheral. `improv-wifi` already contains a working BlueZ advertisement/GATT/application layer, currently private to that crate. Either extract it into something shared or accept duplication; extraction is more work now and less drift later.
- The bestool command itself. `iti` is Tamanu-Iti-specific and this is not, so a new top-level subcommand is probably the right home.

## Open questions

- [ ] Card identifier for this work, so the working doc, plan, and specs land in the right place.
- [ ] Does the device hold the fleet key and derive its own sticker secret, or is the secret derived off-device and installed at imaging time?
- [ ] Where and when are stickers generated and printed, and is there a record of what was printed for which board?
- [ ] What is this called? It needs a name before it needs a crate.
- [ ] Does the QR carry one value or two — a discovery value and a separate challenge secret — and if two, what does the split buy us?
- [ ] Is passive-tracking resistance (rotating handle, private addresses) in scope for the first version?
- [ ] Which provisioning operations are in the first milestone, and is Wi-Fi configuration one of them or does Improv-Wi-Fi keep that job?
- [ ] Does this stay in bestool or become its own project, and does the answer change the crate layout?
- [ ] RPC payload encoding — framing with winnow per house style, but the message bodies could be postcard, CBOR, or hand-rolled.

## Testing notes

- The SMBIOS path is the testable one: a UEFI VM or CI runner has a system UUID where a Pi does not, so the whole derivation chain can be exercised without hardware.
- A board ID override for tests, and a defined behaviour when no board ID source exists at all.
- Full handshake and RPC exercised over an in-memory duplex transport, with no BLE involved.
- Negative cases worth pinning down: wrong sticker secret, replayed advertisement, replayed handshake, truncated frames, a peer that authenticates and then sends garbage.
- Handle collision between two devices in range.
- BlueZ-level testing against a virtual controller is possible but heavy; the protocol core should not need it.
