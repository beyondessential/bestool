# bliti implementation plan

Behaviour is specified under `.workhorse/specs/bliti/`.
This plan carries the implementation choices, what was weighed against them, and the outstanding work.
The reasoning behind the design, including the measurements it rests on, is in `.workhorse/working-docs/u2/working-doc.md`.

## Crate layout

Two crates, both in this repository, standing alone from `bestool` — its own binary rather than a subcommand, with nothing in `bestool` depending on it, which keeps moving it to its own repository later a cheap option.

- **Protocol core.** Board ID reading, the key schedule, the handshake, framing, and message types. No BlueZ and no hardware, so it unit-tests anywhere.
- **Daemon.** Ties the core to `bluer`, plus sticker generation.

Board ID reading sits in the core as backends behind a trait — TPM, one-time-programmable memory, Raspberry Pi serial, SMBIOS, and a test backend — rather than as its own crate. It can move out if something else needs it.

The core earns its separation because the web test page needs the same handshake in the browser. Compiling it to wasm keeps one implementation of the key schedule rather than a Rust one and a JavaScript one that must agree forever. This constrains the core to stay free of I/O and of anything that will not build for `wasm32-unknown-unknown`.

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

Browsers are GATT-only, and the web page is the first milestone's only client. L2CAP does not replace GATT in any case — a connection-oriented channel is identified by a PSM the client must first read from a characteristic, so GATT remains the way in. And nothing in the first milestone moves enough data to notice the difference.

Adding L2CAP later is cheap in a way the advertisement is not: a GATT service is discovered by UUID, so a characteristic publishing a PSM can appear whenever it is written, and clients that do not know about it ignore it. The end state is both, chosen per client, with an identical stack above.

### Streams, and why not QUIC

The property wanted is QUIC's — either end opens a stream without coordinating — and it is the part of the design expensive to change later, so it belongs in the first milestone. But QUIC itself does not fit:

- **The datagram floor.** QUIC requires a path carrying 1200-byte datagrams; the attribute protocol tops out well below that and negotiates lower in practice.
- **TLS does not detach.** QUIC's packet protection comes out of the TLS 1.3 key schedule, so a full TLS exchange would run after Noise has already authenticated both ends and produced a key.
- **Reliability over reliability.** BLE is already reliable and ordered, so QUIC's streams would not escape head-of-line blocking — the blocking happens underneath regardless — while paying for loss detection and congestion control that duplicate the link.
- **Browsers cannot send UDP.** Compiling a QUIC stack to wasm does not rescue the web page.

What delivers the property is a stream multiplexing layer inside the Noise channel, which is QUIC's stream layer with the transport machinery removed. `yamux` is exactly that. Failing it, hand-rolling is a few hundred lines, and QUIC's identifier convention is worth copying either way: low bits encoding which side opened the stream and whether it is unidirectional, so both ends allocate from disjoint spaces.

## Outstanding risks

**TPM seed stability is unverified, and it is the failure that would kill a model line's stickers at once.** The endorsement seed survives the ordinary clear operation, but a platform-authorised command exists to change it, and what a given firmware does on a BIOS-level TPM reset, on disabling and re-enabling a firmware TPM, or across a firmware update, has not been tested. A discrete TPM is far better placed — its seed is fixed at manufacture and attested by a vendor certificate, with no BIOS path reaching it — so the exposure is concentrated on machines with firmware TPMs. Test this on the hardware to be shipped before any sticker is printed for it.

**The derivation is killed rather than failing** when memory is short, so the pre-flight check or child-process isolation in `BLI-KEY` is required rather than defensive.

**BlueZ userspace is not installed on the current device image**, so it is a deployment prerequisite rather than something to assume.

## Milestones

1. **A channel.** Board ID reading, both derivations, sticker generation, advertising a rotating handle, the `NNpsk0` handshake, and a stream layer over GATT carrying JSON, plus the web test page driving all of it. Two things ride on it: a line of text from the browser that the device prints, proving the client-to-device direction, and the device's hostname and addresses, proving the other. Everything genuinely novel is here; what follows is operations on a pipe that already works.
2. **Wi-Fi.** Joining a network, and putting the device into access-point mode — the case Improv cannot express and the reason this protocol carries Wi-Fi at all. The `WifiConfigurator` trait and NetworkManager backend in `improv-wifi` are a starting point, though the access-point case reaches past what that trait expresses.
3. **The rest of provisioning.** Device description — model, board ID, OS image, software versions, network name, local time, battery, temperature. Physical identification, by blinking an LED or drawing on a display where one is fitted. Hostname and timezone. Enrolment with a server. Recent logs or health output. Reboot.
4. **A native application.** Android or iOS, once the protocol has stopped moving.

## Deferred, and free to defer

Advertising only in a window, and waking a quiet device over the air, are carried as draft cards in the card breakdown.

**Multiple concurrent versions.** The current device controller supports one advertising set, so a device advertises one version at a time and would have to alternate to support several, at a cost in discovery latency. Nothing needs building now; reserving the version marker in both payloads is what keeps it available, and that is in the first milestone because adding it later breaks every deployed device.

## Build order

- [ ] Board ID backends behind a trait: Raspberry Pi serial, SMBIOS, TPM Endorsement Key, one-time-programmable memory, and a test backend
- [ ] Source precedence, sentinel rejection, and the failure when nothing is usable
- [ ] Key schedule: both derivations, the version marker, and known-answer tests pinning them
- [ ] Sticker secret caching, cache invalidation against the board, and the pre-flight memory check
- [ ] Sticker generation, QR payload encoding, and the human-readable rendering
- [ ] Noise `NNpsk0` handshake over an in-memory transport
- [ ] Framing and reassembly
- [ ] Stream layer over the framed transport
- [ ] JSON message types
- [ ] GATT server and characteristics via `bluer`
- [ ] Advertisement and scan response construction within the 31-byte budgets, with salt rotation
- [ ] Daemon tying it together, running as a service
- [ ] Address and hostname reporting, including unsolicited sending on change
- [ ] Text echo to standard output
- [ ] Web test page driving scan, match, handshake, and both directions
