---
status: complete
---

# bliti: QR-anchored BLE device provisioning

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

Everything is derived from fixed public constants and the board ID. There is no fleet key and no authoritative per-device state: the chain is reproducible from the board alone, at manufacture or any time after. Anything the device stores is a cache it can rebuild. This is a deliberate choice, and it sets what the scheme can and cannot buy.

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

Measured on a Dell G7 7700, the SMBIOS UUID is `4c4c4544-0042-4710-804d-b3c04f485832` and carries nothing like 128 bits. The leading four bytes are the ASCII marker `LLED`, and six of the seven characters of the service tag `3BGMHX2` appear verbatim as ASCII bytes in the remainder. The UUID is a formatting of the service tag, whose space is about 36^7, or roughly 2^36 — and less than that in practice, since service tags are not uniformly random. The worst case in the paragraph above is not hypothetical: it is the first machine we looked at.

**Decided: the board ID is the strongest source available, under a fixed precedence, rather than a combination of all of them.**

Combining was the earlier answer, on the reasoning that if no single source can be trusted then pooling them is the best available move. A TPM Endorsement Key on UEFI machines and burnt customer OTP on Raspberry Pi both carry 256 bits, and once such a source is in hand, combining a weak one alongside it adds nothing measurable while making that weak source load-bearing. Every source in a combination is a way for the board ID to change, and a board ID that changes orphans a sticker that is already on an enclosure.

The precedence is by **kind of source, not by platform**: TPM Endorsement Key, then provisioned OTP, then the platform's serial numbers. Keying it to the kind rather than the board means a Raspberry Pi that later gains a TPM over SPI needs no new rule — it simply has a stronger source than it had before. It also means the device and the sticker generator reach the same answer without being told which they are, since the precedence is evaluated against what is actually present.

The corollary is the thing to be careful about. Adding a stronger source to a board that already wears a sticker changes which source wins, and so changes the secret: fitting a TPM to a deployed Pi, or burning OTP after the fact, orphans that board's sticker. A hardware change of that kind means a reprint, and the device should be able to say that is what happened rather than leaving an operator to discover it by a scan that never matches.

#### The TPM Endorsement Key

A TPM 2.0 holds a far better identifier than anything in SMBIOS, and it is not only a PC-grade part: a discrete TPM over SPI puts the same source on a Raspberry Pi. The Endorsement Key is generated from the Endorsement Primary Seed under a standard, published template, so it is the same key every time it is asked for. Its *name* — the hash algorithm identifier followed by the SHA-256 digest of the public area — is a compact fixed-length value carrying the full entropy of a real 2048-bit RSA public key, and it is readable without authorisation.

**The template is pinned to the TCG low-range RSA 2048 Endorsement Key.** A TPM holds several Endorsement Keys, one per algorithm, so "the Endorsement Key" is ambiguous until the algorithm is named — and naming it wrongly later changes every board ID derived under the old choice. RSA 2048 is the common denominator: it is present on the Intel firmware TPM tested here, and on both firmware families of the discrete part below. This is as load-bearing as the derivation constants, and moves for the same reasons and with the same consequences.

Verified on the same Dell, against its Intel firmware TPM. Regenerating the Endorsement Key from the standard template twice produced byte-identical names, and both matched the key already persisted at the conventional endorsement handle: `000b4e511a6e9753d54b298cfa8f84bb3aa6f7f900673035b0cec2bc3f5318de126d`. So the value is derivable from the board alone rather than read from somewhere it was stored, which is the property the whole scheme rests on — and it needs no provisioning step at all, unlike the Pi OTP option below.

Regenerating rather than reading a persistent handle is the right way to obtain it. The persistent handle is populated by provisioning software and is not guaranteed to exist on a freshly imaged machine, whereas the seed and the template are always there.

Two risks, neither settled here — and the first lands almost entirely on the firmware-TPM case rather than the discrete one.

The Endorsement Primary Seed survives the ordinary clear operation, which resets the storage hierarchy, but a platform-authorised command exists to change it outright. What a given firmware does on a BIOS-level TPM reset, on disabling and re-enabling a firmware TPM, or across a firmware update, was not tested and should not be assumed. If the seed changes, the key changes, and the sticker is orphaned. A discrete TPM is far better placed here: its seed is fixed at manufacture and attested by a certificate the vendor issues against it, and there is no BIOS operation reaching in to regenerate it. The exposure is therefore concentrated on PC-grade boxes with firmware TPMs, which is also where the fallback is weakest.

Reading it costs a dependency. The operation needed is narrow — create a primary key under the endorsement hierarchy with the standard template, then read its name — but reaching it from Rust means either the established bindings to the TPM software stack, which carry a C library, or speaking the TPM command protocol to the resource manager directly. Either way it belongs behind the board-ID backend interface and out of the core, which has to keep building for wasm.

##### A discrete TPM on Raspberry Pi

The part in view is Infineon's OPTIGA SLB 9672, a TCG-compliant TPM 2.0 reached over SPI. It ships factory-provisioned with Endorsement Keys and matching certificates — three on the FW15 family, four on FW16, the extra one being RSA 3072 — with the certificates held at the standard NV indices and valid for fifteen years from production. RSA 2048 is present across both, which is what makes it safe to pin above.

Linux binds it through the generic TPM SPI driver; the device tree declares the part and falls back to the earlier SLB 9670 binding so the existing driver attaches, and Raspberry Pi OS carries an overlay for it already. Boards following the Pi HAT specification load that overlay themselves. So this is an ordinary `/dev/tpm` device once fitted, and the board-ID backend does not care which kind of TPM it is talking to.

Two things follow that are worth having in mind before it is fitted rather than after. A Pi that gains a TPM gains a source that outranks the serial it was using, which is the orphaning case described above, and matters most for boards already in the field. And the certificates make a stronger claim available than the board ID needs today: they chain the key to the vendor, so a device could later prove it is genuine hardware rather than merely consistent. Nothing in the first milestone uses that, but deriving from the Endorsement Key rather than from something invented keeps it available.

#### Provisioned entropy on Raspberry Pi

Raspberry Pi SoCs carry a customer-programmable one-time-programmable area: eight rows of 32 bits, 256 bits in total, at rows 36–43 on non-BCM2712 parts and rows 77–84 on BCM2712, which is the Pi 5. There is also a 256-bit device-specific private key area at rows 56–63, but it is listed only for non-BCM2712, so the customer rows are the portable choice across the boards we would ship. Writes are irreversible in the usual OTP sense — bits go from 0 to 1 and not back — and row 30 holds bits that can disable OTP programming and reading altogether.

This is a way out of the entropy problem on the Pi side rather than a mitigation of it. If production burns 256 bits of true random into the customer area, the Pi path stops depending on a 32-bit OTP word or a 64-bit serial and carries 256 bits that were chosen rather than inherited. Under the precedence above it sits below a TPM and above the platform serials, so a Pi with burnt OTP and no TPM derives from the OTP, and a Pi with neither falls back to its serial and the weak-source case it implies. Whether production adopts the burn is settled outside this card, and nothing here waits on it: bliti reads the area where it has been written and falls through to the next source where it has not, so both outcomes are already handled.

Two consequences, both about ordering and neither optional.

Blank OTP reads as zeros, which is a sentinel rather than a value, so an unburnt board must fall through to the next source rather than derive 256 bits of zero. See "Sources that are not identities".

And the burn has to happen before the secret is derived and the sticker printed. Burning afterwards promotes OTP above the serial the sticker was derived from, which silently orphans it — and because OTP is irreversible there is no putting it back. The board then wears a sticker that no longer matches it until someone reprints.

#### Sources that are not identities

A source can be present and still carry no identity. A value that reads as all zeros, all ones, or a known vendor constant is a placeholder whatever its nominal width, and hashing it as though it were a value gives every board in the same position the same secret. Unburnt OTP is the ordinary case — it reads as zeros on every unprogrammed board — and a vendor that ships a constant SMBIOS UUID across a model line is the other.

Such a source is skipped, and precedence falls through to the next one. That is the difference between this and a hard failure: a Pi with unburnt OTP is not broken, it simply derives from its serial instead, which is the weak-source case and a supported one.

Reaching the end of the precedence with nothing usable is the failure, and it fails loudly rather than deriving from a placeholder.

bliti targets bare metal, and detecting virtualised or cloned machines is out of scope for now. It is worth knowing why the question arises, so that the scope can be revisited deliberately rather than by surprise: a software TPM takes its seed from the image it was cloned from, so machines stamped from one golden image would share an Endorsement Key, and therefore a board ID and a sticker secret.

#### Boards with only their serial numbers

**Decided: a board whose strongest source is its platform serial numbers is shippable.** Devices already in the field have neither a TPM nor burnt OTP, and they are in scope — bliti has to work on the hardware we have shipped, not only on hardware we would specify now.

What that costs is worth stating rather than leaving for the derivation to imply. Against a source of around 2^32 to 2^36, a memory-hard derivation raises the price of enumerating the whole space but does not put it beyond someone willing to spend real money on a particular device. Boards in this class therefore carry a materially weaker guarantee than a board with a TPM or burnt OTP, and the threat model should say so. The strong-source boards are not incrementally better; they are in a different class, and the derivation cost cannot flatten that difference however hard the parameters are pushed.

Selecting the strongest source rather than combining them is what leaves the door open to raising the floor later. Under a combination, every board's identity depends on every source it has, so changing the rule changes every board ID at once and orphans the entire field. Under precedence, a board that has a TPM today already derives from it, so declaring weak sources unacceptable tomorrow changes nothing for it.

Where such a floor would apply needs care. Refusing a weak source at sticker generation stops new devices being provisioned from one, which is the intent. Refusing it in the device daemon would strand exactly the devices the floor was introduced to move past, since they derive their handle at runtime from the only source they have. The floor belongs at manufacture; the daemon keeps deriving from whatever the board offers.

### Sticker secret

The value carried in the QR code, and the only secret in the system. Derived from the board ID under a fixed constant.

The constant is public — it can be compiled into the device, the sticker generator, and the phone app without weakening anything, because a hash does not run backwards. What the constant buys is domain separation, not secrecy.

The board ID does **not** go in the QR alongside it. Putting it there would hand the board ID to anyone who photographs a sticker, which is precisely the property the derivation exists to provide.

Because the secret is per-board and derived, compromising one device yields that device and no other. There is no fleet-wide value to leak.

### Cost of deriving the sticker secret

With a public constant, the only thing stopping someone enumerating the board-ID space offline — computing every sticker secret, every handle, and matching them against advertisements they can hear — is the cost of the derivation. That attack needs neither the sticker nor access to any device, so it is not covered by what the threat model puts out of scope.

A plain hash makes it cheap: 32 bits of board ID against a fast hash is minutes of GPU time, and 64 bits is not a comfortable margin either.

How much this section still has to carry depends on what the board ID turns out to be. It was written assuming the board ID is whatever firmware happens to expose, which is the weak case measured under "Board ID" — around 2^36 on the Dell. Where a TPM Endorsement Key or burnt OTP wins the precedence instead, the board ID carries 256 bits, nothing can enumerate it, and the memory-hard derivation is defence in depth rather than the thing holding the scheme up. The argument below therefore sizes the derivation for the weakest source we are willing to ship, not for the typical one.

Making the first derivation **memory-hard** — argon2id or scrypt rather than a plain hash — closes this without changing the model. The constant stays public and the derivation stays reproducible from the board alone; only the cost of doing so moves. Stickers are generated once at manufacture, so the generator can afford whatever we ask of it.

Parameters should be pushed hard, to seconds of work rather than milliseconds. The useful mental model is that an attacker's cost per guess scales with memory times iterations, and a GPU's parallelism is capped by its VRAM divided by the memory parameter — so memory buys more than time does, since it bounds how many guesses can run at once rather than just how long each takes.

**Where the derivation runs.** The sticker generator runs it at manufacture, on the machine driving the sticker printer, which can be as large as we like. The device also needs the secret at runtime, to compute its handle and run the handshake — so the derivation has to be runnable there too, unless the device is simply handed the result.

The target is a Raspberry Pi 5 with 8 GB, planning against a 4 GB floor, and that is roomy enough that the two do not conflict. A memory parameter the printer machine finds trivial is still well within a 4 GB board's reach, so the device can derive its own secret and the "rederive from the board, no records needed" property holds everywhere rather than only on big machines. Choosing a parameter larger than the floor would give that up permanently: recovery would then need a machine big enough to run it, with the board ID read off the device first.

Headroom is not the same as free. The derivation should be paid once per imaging and **cached**, not repeated at every boot, and the parameter should leave room for whatever else the board is running rather than claiming most of its RAM. The cache is an optimisation and not a source of truth: if it is absent, or does not match the board it finds itself on — an SD card moved between boards — the device rederives.

The second derivation, sticker secret to advertised handle, stays a fast hash either way. The phone compares against every advertisement it hears while scanning, and a memory-hard function in that loop would be felt.

#### Measured parameters

**Decided: argon2id, 2 GiB of memory, one pass, two lanes.** Measured on a Raspberry Pi 5, which is the slowest board in scope: 2.4 seconds there, and around a second on a desktop-class generator.

Two things came out of the measurement that are not obvious from the parameter space.

Four lanes are slower than two on this hardware — 2.41 s against 2.37 s at this setting, and 5.78 s against 5.62 s at three passes — because four cores saturate the memory bus long before they run out of work. Lanes are part of the parameter set and change the output, so choosing four would have baked in a permanently worse value for nothing.

And memory is the right axis to spend on. At equal or better wall time, 2 GiB with one pass beats 1 GiB with three, because an attacker's parallelism is bounded by memory divided into their card's, while passes only make each guess longer.

Concurrency is not part of the parameter set. The same parameters produce identical digests whether lanes are computed in parallel or in sequence, verified across one, two, and four lanes. So the device, the sticker generator, and any future implementation agree on the value regardless of how each chooses to compute it — the speed knob and the key schedule are separate.

**What the cost actually buys, against each source.** Taking a datacentre GPU at roughly 2 TB/s of memory bandwidth and around 6 GiB of traffic per guess, an attacker gets on the order of hundreds of guesses per second per card. These are order-of-magnitude figures, but the conclusion does not depend on the precision:

| source | space | one card | a hundred cards |
| --- | --- | --- | --- |
| pre-Pi-4 serial | 2^32 | months | days |
| vendor-structured SMBIOS UUID | 2^36 | years | weeks |
| Pi 4 and 5 serial | 2^64 | infeasible | infeasible |
| TPM Endorsement Key, burnt OTP | 2^256 | infeasible | infeasible |

So the weak class is narrower than "platform serials" suggests. A Pi 5 serial is 64 bits and genuinely safe — measured on hardware as `f3756510f632cfad`, with the high half populated rather than zero. What is weak is specifically the pre-Pi-4 boards, where the serial collapses to a 32-bit word, and vendor-structured SMBIOS UUIDs of the kind measured on the Dell. No parameters tolerable on a boot path rescue either; the derivation cost raises the price without moving the outcome.

**The derivation does not fail gracefully when memory is short.** Peak resident memory is 2057 MiB, a hair above the parameter. Under a cgroup limit it runs at full speed down to 2100 MiB and is killed outright below that — the kernel overcommits, so the allocation succeeds and the process is killed when it touches the pages. Exit by signal, no error to catch, nothing reported.

That is a requirement rather than a curiosity, and it falls out of choosing 2 GiB against a 4 GB floor. The daemon establishes there is room before it starts, and reports that there is not rather than being killed mid-derivation; or it derives in a child process, so a kill is something it can observe and report instead of the daemon's own death.

### Advertised handle

A one-way hash of the sticker secret, truncated to a handful of bytes, broadcast in the BLE advertisement.

- The derivation constant here need not be secret. Hashing is one-way, so a listener who hears the handle still cannot recover the sticker secret; a phone app can carry the constant in the clear.
- The handle must be long enough that collisions across a site are implausible, and short enough to leave room in a 31-byte legacy advertisement. Eight bytes is a comfortable default.
- A scanner matches by recomputing, so the cost is one fast hash per advertisement seen per sticker it holds. Fine for a phone looking for one device; an app holding a few hundred stickers is doing a few hundred hashes per advertisement, which is still cheap but is the reason this step is not memory-hard.
- Stickers outlive software. Once a box is in the field its sticker is fixed, so a later change to the constants or the derivation parameters must not orphan it: the payload carries a version, and the advertisement carries it too. See "Versioning the key schedule".
- A **static** handle makes the device passively trackable — a fixed beacon following the box around. See "Tracking resistance".

#### What the target controller actually supports

Measured on the Pi 5's own controller, which is a Bluetooth 5.0 part from Cypress. Two of its feature bits decide more of this design than any choice we make.

**No extended advertising.** The controller reports no support for it, and none for the 2M or coded PHYs either. So there is one advertising set and one legacy payload of 31 bytes, and the larger extended payloads are not available. The doc's 31-byte assumption was right, and it is a hard floor rather than a conservative starting point. It also means a device cannot advertise several handles as several sets, because there is only one set to register.

**No LL privacy, and a resolving list of size zero.** Controller-assisted private addressing is not present on this hardware at all. The same desktop that does support it still produced no private address under BlueZ, so nothing here changes the decision already taken — but it removes any expectation that the target hardware could do better. Address privacy stays the host's business, and the guarantee stays narrowed to which device rather than same device.

**The 31 bytes are tighter than they look.** A 128-bit service UUID, which iOS needs in order to filter a scan at all, costs 18 of them as an advertising structure, and the flags structure costs another three. That leaves ten bytes in the advertisement itself, which a handle, a salt, and a version marker do not comfortably fit into. The scan response is the way out: a second 31 bytes, returned to an active scanner, which both iOS and Web Bluetooth merge with the advertisement before the application sees it. The service UUID stays in the advertisement, where scan filtering can see it, and the service data carrying handle, salt, and version goes in the scan response.

#### Versioning the key schedule

Everything the sticker depends on is versioned together under one marker, carried in the QR payload and in the advertisement: the derivation constants, the argon2 parameters, the source precedence and its encoding, the pinned Endorsement Key template, and the handle length. Any of those moving is a new version, because any of them moving changes the secret.

The asymmetry is what makes this cheap to adopt and expensive to use. A client holds the sticker, so it reads the version and derives once. A device holds no sticker and cannot know which version was printed for it, so supporting several means deriving under each — 2 GiB and 2.4 seconds apiece, cached — and advertising under each, which the single legacy advertising set above does not allow simultaneously.

So a device advertises one version at a time. Supporting more than one would mean alternating between them, at a cost in discovery latency proportional to how many. That is a workable answer if it is ever needed, and nothing about it has to be built now: reserving the version marker in both payloads is what keeps it available, and reserving it is free today and impossible later.

### Tracking resistance

A device that advertises is a beacon, and the question is how much a passive observer with no sticker can learn. Four things leak, and they are not independent — the weakest one sets the result, so partial measures buy nothing.

**The BLE address.** If the adapter advertises from its public static address, the device is trackable outright and nothing else matters. The fix is LE Privacy: resolvable private addresses, which the controller rotates on its own. Since matching is by payload rather than by address, no peer needs to resolve them and there is no bonding to arrange. The catch is that this is adapter configuration rather than something the protocol controls, and checking what BlueZ and `bluer` actually expose turned out to matter a great deal — see below.

Checked, on BlueZ 5.87 with a controller that advertises LL privacy support (25-entry resolving list, LE feature bit set). `bluer` exposes no privacy control at all: `AddressType` is a read-only adapter property, and `bluer`'s own documentation for it notes that with privacy enabled it reports the type of the identity address rather than the address in use. Privacy is `Privacy = off|network/on|device|…` under `[General]` in BlueZ's `main.conf`, defaulting to `off`. Setting it to `network/on` and restarting produced no local IRK and no resolvable private address, across both a normal restart and a restart with the adapter powered down, with nothing logged by `bluetoothd`. Whether that is a misconfiguration, a `bluetoothd` fault, or an API gap was not established — seeing the over-air address needs `btmon`.

**The handle.** A fixed handle defeats address rotation by itself. Rotating it needs no clock — which matters, because a device that has never had network has no idea what time it is. Advertise a short random salt in the clear alongside `H(k2, sticker secret, salt)` and roll the salt periodically: a scanner recomputes for whatever salt it observes, and an observer without the sticker secret cannot link two advertisements. Costs a few bytes.

**Rotation has to be in lockstep, and cannot be.** If the address rolls every fifteen minutes and the handle every hour, an observer bridges each address change using the handle, and the address rotation was wasted. Same in reverse. They should roll on the same event — and no such event is available to us. `bluer` reports only the identity address; the live private address appears in kernel debugfs, which is an internal interface rather than a contract, so the daemon has no supported way to learn that the controller has rolled.

Matching the *periods* does not substitute for sharing the *event*. With the address rolling at t = 0, 900, 1800 and the salt at t = 450, 1350, an observer links the two salts across t = 450 because the address did not change there, and links the two addresses across t = 900 because the salt did not change there; the chain joins end to end and neither rotation achieved anything. Two independent timers of equal period are as useless as timers of different periods unless they are also phase-aligned, which nothing aligns them.

**Decided: the device rolls the salt, and the address is the host's business.** Address privacy becomes a documented requirement on the adapter rather than something bliti arranges, and the guarantee narrows accordingly. What bliti claims is that an observer without the sticker cannot tell *which* device it is hearing. It does not claim that such an observer cannot tell it is hearing the same device twice, because on a host where privacy is off the static address links everything regardless of what the payload does. Saying so plainly is better than implying a property the stack will not deliver.

The alternative — going below `bluer` to the kernel management socket, setting the adapter's random address explicitly, and rolling it together with the salt as one operation — would buy real lockstep. It was weighed and dropped for milestone one: it puts a BlueZ plumbing layer back on our maintenance surface, which is the thing choosing `bluer` was meant to avoid. It stays available if tracking resistance is ever wanted as a guarantee rather than a best effort.

**The service UUID** identifies the device as one of ours to anyone who knows to look, and it cannot be hidden: iOS can only filter a scan by service UUID, so it has to be there in the clear. This is a fleet-level fact rather than a per-device one — an observer learns "a bliti device is here", not which one. Accept it.

#### Rotation period

The salt rolls on a timer. Rolling it means re-registering the advertisement, which costs one round trip and a brief gap in advertising; clients recompute against whatever salt they observe, so a shorter period costs a scanner nothing.

**Decided: fifteen minutes.** It matches the conventional private-address rotation period, so on a host where address privacy is configured the two at least share a period even though nothing can align their phase. With lockstep unavailable the exact figure carries little weight, and a familiar number needs no defending.

#### Advertising continuously

The device advertises all the time. Rotation of both the salt and the address is what makes that acceptable, and both are in from the start.

The stronger measure — advertising only for a window after boot, then going quiet — is deliberately deferred. `improv-wifi` does something like it, staying silent once provisioned and waking on a button long-press; bliti has no button, but a device with no button still has a power cable, and a power cycle needs someone standing at the box just as a button press does. That makes it available as a presence signal whenever we want it.

It is deferred rather than dropped because it costs nothing to add later. It is local policy — when the daemon chooses to advertise — with no bearing on the wire format, the handshake, or anything an app has to understand. The only client-side consequence is telling an operator to power-cycle a device that is not answering. So it stacks on top of the protections rather than replacing them, and taking the convenient option now forecloses nothing.

#### Waking a quiet device over the air

A power cycle is not the only way to open a window. A quiet device could scan for a wake beacon from the client instead, which is far more convenient than asking someone to unplug a box, and still leaves the device silent the rest of the time.

**Targeted, not broadcast.** A wake anyone can send, that every device in range answers, is an open presence oracle: sweep a building and map where devices are, on demand. Rotation keeps that to "a device is here" rather than naming which — the same fleet-level leak the service UUID already gives away — but it turns a passive leak into one an attacker can probe at will.

Deriving the wake signal from the sticker secret closes that. The client advertises a salt alongside `H(k3, sticker secret, salt)` — a third domain separator, the same shape as the handle — and only the device whose secret matches responds. Someone without a sticker can wake nothing. The device checks one hash per beacon it sees, against its own single secret. This is also the normal case: a client that has scanned a sticker knows which device it wants. Waking without one is a capability that only helps whoever should not have it.

**What the client platforms allow** decides the encoding, and is the reason this cannot be built yet:

- Android advertises service data without difficulty.
- iOS does not. `CBPeripheralManager` accepts only a local name and service UUIDs in an advertisement — no service data, no manufacturer data. The way through is to carry the whole wake token *as* a 128-bit service UUID, which iOS advertises happily. The device then scans passively and inspects candidate UUIDs itself rather than filtering on one, since it cannot precompute a salt the client chose.
- Web Bluetooth cannot advertise at all. It is central-only.

That last point settles the sequencing. The web page is the first milestone's only client, so it can never send a wake beacon; this waits for a native app. Deferring stays free, because the wake channel is separate from the device's own advertisement and `k3` is just another constant — nothing needs reserving in the wire format now.

Scanning costs more power than advertising, which is immaterial on a mains-powered Pi 5 and would want duty-cycling on anything battery-backed: a short scan every several seconds, against a client that advertises for long enough to span the gap.

What continuous advertising does cost, beyond exposure: anyone in range can open connections and start handshakes that will fail. That is cheap to absorb — the handshake's key exchange is microseconds — but it shapes two choices. **There must be no lockout after repeated failures**, or someone in range could deny the device to its legitimate operator, which is a worse outcome than the attacks a lockout would prevent. And failed attempts must not be logged so freely that a passer-by can fill the disk.

The wire format carries the rotation salt from the first milestone regardless, since adding it later breaks every deployed device and app.

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

Once authenticated, a two-way, ordered, reliable message channel carries a small request/response RPC.

**The channel itself is the first milestone**, not any particular operation on it. Getting discovery, authentication, and a working bidirectional pipe is the hard and interesting part; the operations are comparatively ordinary once it exists.

The demonstration that the channel works: the web test page sends a line of text and the device prints it. Printing to stdout rather than broadcasting to terminals keeps it visible in both places it will be watched from — a developer running the daemon over ssh sees it directly, and once it is a systemd unit the same line lands in the journal with no extra work.

Messages are JSON. The volumes here are tiny, every client platform reads it without a library, and being able to watch the conversation in plain text is worth more during development than any saving a compact encoding would give.

**The device must be able to send unsolicited messages**, not only answer requests. Status that changes while a client is connected — an address appearing as Wi-Fi comes up, which is precisely what an operator is watching for while provisioning — should arrive as it happens rather than by polling. This is what the stream layer under it exists to provide: the device opens a stream of its own rather than waiting to be asked. See "Streams, and why not QUIC".

### Standing in for the screen

The Iti carries an LCD whose entire purpose is to show an operator standing in front of it a few facts about the device. `services/iti-addresses` polls every sixty seconds and draws the device's `.local` hostname and its IPv4 addresses; `services/iti-lcd-wifi` draws the connected network's name; other units draw local time, battery, and temperature.

Carrying the same facts over the channel makes that hardware optional. The phone becomes the screen, for a device that never needed one — which is a saving on every unit, not just a convenience.

It is also **more private than the screen it replaces**. Anything on an LCD is readable by anyone who walks past it, including the device's addresses and network. Over the channel the same facts require having scanned the sticker.

**Addresses are in the first milestone**, since they are the fact an operator most often wants and they prove the channel carries something real. The screen's shape should not be copied, though: its limits are display limits, not information limits. A 240-pixel panel is why `iti-addresses` stops at three addresses, wraps at twenty-six characters, skips IPv6 entirely, and filters podman interfaces out by name. The channel has none of those constraints, so the device should send every global address it has with the interface each belongs to, and let the client decide what is worth showing. Keeping the `up` and `global` filtering is right — loopback and link-local addresses are noise in any presentation.

The rest of the screen's contents — network name, local time, battery, temperature — follow the same argument and belong with the device description operation.

Wi-Fi configuration is where this is headed, and the reason it does not simply defer to Improv-Wi-Fi is that we want **more** than Improv's model allows — notably putting the device into access-point mode rather than only joining an existing network. Improv's RPC has no vocabulary for that. So Wi-Fi becomes an operation on this channel, and Improv-Wi-Fi keeps its own separate life for whatever still wants to speak Improv.

Candidate operations after the channel exists:

- Configure Wi-Fi, including switching the device to an access point. The `WifiConfigurator` trait and NetworkManager backend in `improv-wifi` are a starting point, though the AP case reaches past what that trait currently expresses.
- Describe the device: model, board ID, OS image, software versions, and the rest of what the Iti screen shows — network name, local time, battery, temperature.
- Physically identify: blink an LED, or draw on the LCD where one is fitted, so the operator can confirm which box they are talking to.
- Set hostname, timezone.
- Enrol the device with its server.
- Fetch recent logs or health output for field diagnosis.
- Reboot.

### Web test page

A static page using Web Bluetooth that does the scan-match-authenticate-RPC flow, so the protocol can be exercised before any native app exists. Chrome on Android and desktop only; Safari and Firefox have no Web Bluetooth, and on iOS it requires a third-party browser shell. Good enough for development, not a shipping story.

## Implementation options

### Choice of derivation function

**argon2id** for the first step: the current default recommendation, with a maintained pure-Rust implementation, and a better memory-hardness margin than scrypt's. Parameters are three-dimensional — memory, iterations, parallelism.

For the fast second step, BLAKE3 keyed mode gives domain separation and a truncatable output in one primitive.

Parameters want measuring on a Pi 5 before they are fixed, against the 4 GB floor and with the board's other work in mind. This belongs before anything is printed on a sticker — the constants and parameters are baked into every sticker in the field the moment one ships.

### Handshake

- **MAC challenge-response.** Each side sends a nonce; each replies with a MAC over both nonces under the sticker secret, domain-separated by direction. Mutual, replay-resistant, trivially implementable and auditable. No session key worth the name and no forward secrecy — anyone who later learns the sticker secret can decrypt a recorded session.
Decided: **Noise with a pre-shared key**, pattern `NNpsk0`. The rest is kept as the record of what was weighed.

`NNpsk0` means both sides bring only ephemeral keys and all authentication comes from the pre-shared sticker secret — which is the whole design, since everything derives from the board ID and there is no other identity in the system. The alternative, `XXpsk0`, would additionally give the device a long-term keypair of its own, so a phone could remember "this device" independently of its sticker. That only earns its keep if a device's sticker secret can change while the device stays the same, and here it cannot: the secret is a function of the board.

- **Noise with a pre-shared key** (`NNpsk0`, via `snow`). The sticker secret is the PSK. Mutual authentication from the PSK, a fresh session key and forward secrecy from the ephemeral DH, an encrypted transport falls out of it, and the pattern is well-analysed. Adding a device static key later lets a phone pin device identity across a sticker reprint.
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
- **L2CAP connection-oriented channels.** A real stream with far better throughput; supported by BlueZ, iOS 11+, and Android API 29+. `bluer` exposes it behind a feature flag.

**GATT first, L2CAP later**, for three reasons that stack rather than compete.

Web Bluetooth is GATT-only — the API has no L2CAP at all. The web page is the first milestone's only client, so a device speaking L2CAP alone would have nothing to talk to.

L2CAP does not replace GATT in any case. A connection-oriented channel is identified by a PSM, and the client has to learn which one; the usual arrangement is to publish it in a GATT characteristic and open the channel after reading it. GATT remains the way in, and building it now is a prerequisite rather than a detour.

And nothing in the first milestone moves enough data to notice the difference. A line of text and a handful of addresses are not what L2CAP's throughput is for.

Adding it later is cheap in a way the advertisement format is not. A GATT service is discovered by UUID, so a characteristic publishing a PSM can appear whenever it is written: clients that do not know about it ignore it, and clients that do find it. Nothing needs reserving now — unlike the rotation salt, where a fixed advertisement budget parsed by position means late additions break deployed devices.

The end state is both, chosen per client: a native app reads the PSM and upgrades, the web page stays on GATT, and the stack above is identical either way.

### Streams, and why not QUIC

The property wanted from the transport is QUIC's: once the handshake is done, **either end can open a stream**, unidirectional or bidirectional, without asking permission or coordinating identifiers, and several can be in flight at once. It is a property worth having, and it is the part of the design that is expensive to change later, so it belongs in the first milestone rather than being discovered during the second.

It is also separable from QUIC itself, which is fortunate, because QUIC does not fit here.

- **The datagram floor.** QUIC requires the path to carry datagrams of at least 1200 bytes and will not operate below that. ATT tops out at 517 bytes and negotiates lower in practice. Running QUIC over GATT means building a fragmenting datagram layer underneath it purely to satisfy a minimum the link cannot meet.
- **TLS does not detach.** QUIC's packet protection keys come out of the TLS 1.3 key schedule; there is no standardised QUIC without it. Certificate validation can be ignored, but the handshake still runs — a full TLS exchange after Noise has already authenticated both ends and produced a session key.
- **Reliability over reliability.** BLE, whether GATT or an L2CAP channel, is already reliable and ordered. QUIC's streams escape head-of-line blocking only when the layer beneath can deliver out of order; over an ordered pipe the blocking happens underneath regardless. We would get the stream API without the property that motivates it, and pay for loss detection and congestion control duplicating what the link already does.
- **Browsers cannot send UDP.** Compiling a QUIC stack to wasm does not rescue the web page: there is no datagram path out of a browser, and Web Bluetooth offers a GATT characteristic, not a socket.

What delivers the property instead is a **stream multiplexing layer inside the Noise channel** — which is, in effect, QUIC's stream layer with the transport machinery removed, because the transport machinery is what the link is already doing.

[`yamux`](https://github.com/paritytech/yamux) is exactly this: "multiplexer over reliable, ordered connections", MIT/Apache, maintained, widely used through libp2p, with flow control already solved. It carries no I/O of its own, so it should build for wasm — which it does. **Confirmed: `yamux` 0.14 builds for `wasm32-unknown-unknown` with no coaxing**, and depends unconditionally on `web-time`, the shim that makes `Instant` work in a browser, so browser support is deliberate upstream rather than incidental. Adopted; the stream layer is not hand-rolled.

Failing that, hand-rolling is a few hundred lines, and QUIC's stream identifier convention is worth copying either way: the low bits of the identifier encode which side opened the stream and whether it is unidirectional, so both ends allocate from disjoint spaces and can never collide without negotiating anything.

The layering this settles on:

| layer | what it gives |
| --- | --- |
| BLE GATT, later L2CAP | reliable, ordered bytes |
| framing and reassembly | message boundaries over the ATT MTU |
| Noise `NNpsk0` | mutual authentication, encryption, a session key |
| stream multiplexing | either end opens uni- or bidirectional streams |
| JSON | application messages |

Two things fall out of it. Swapping GATT for L2CAP later changes only the bottom row, leaving everything above untouched — and since the choice is per client, both can exist at once without the layers above knowing which is underneath. And the first milestone's request/response exchange stops being the protocol and becomes one stream among many, which is the point.

### Crate layout

Name for the protocol and its crates: **bliti**, after *Albugo bliti*, the white rust of amaranth. Free on crates.io. Names built around "improv" were considered and dropped: sitting next to our `improv-wifi`, which genuinely does implement that spec, anything similar would read as a second Improv protocol, and this is not one.

It lives in this repository but stands alone — its own binary, not a `bestool` subcommand, and nothing in the `bestool` binary depends on it. That keeps the option of moving it to its own repository later without unpicking anything.

**The BlueZ layer comes from `bluer`**, the BlueZ project's own Rust interface, rather than from anything of ours. Nothing is extracted from `improv-wifi` and that crate is left alone: it keeps its hand-rolled zbus layer and keeps working. Building bliti on `bluer` doubles as an evaluation of it — if it proves good, rewriting `improv-wifi` onto it becomes an easy call to make later on evidence rather than now on speculation. If it proves awkward, only bliti wears that, and `improv-wifi` was never disturbed.

That leaves two crates:

- **The protocol core**: board ID reading, the key schedule, the handshake, framing, and RPC types. No BlueZ and no hardware, so it unit-tests anywhere.
- **The daemon**: the binary that ties the core to `bluer`, plus sticker generation.

Board ID reading sits inside the core as backends behind a trait — Raspberry Pi, SMBIOS, and a test backend — rather than its own crate. It can move out if something else needs it.

The core earns its separation from the daemon for a reason beyond tidiness: the web test page needs the same derivations and the same handshake in the browser. Compiling the core to wasm keeps one implementation of the key schedule rather than a Rust one and a JavaScript one that must agree forever. This constrains the core to stay free of I/O and of anything that will not build for `wasm32-unknown-unknown` — `snow`'s pure-Rust resolver does, and `bluer` stays out of the core precisely so this holds.

### Why bluer, and what it settles

[`bluer`](https://github.com/bluez/bluer) is the BlueZ project's own Rust interface — BSD-2-Clause, so compatible with our GPL-3.0-or-later, actively maintained, and widely used. It covers peripheral advertising and the GATT server, and ships **L2CAP behind a feature flag**, which is the transport binding pencilled in for bulk transfer. Building on it means no BlueZ plumbing on our maintenance surface at all.

The alternative — extracting and generalising `improv-wifi`'s hand-rolled zbus layer so both protocols could share it — was the other candidate. It has the advantage of being a refactor of known-good code rather than a rewrite against an unfamiliar API, but it keeps a BlueZ implementation as ours to maintain, and it means touching a published crate that is running on devices today.

Taking `bluer` for bliti alone sidesteps that trade entirely for now. `improv-wifi` is untouched, so nothing in the field is at risk, and bliti's experience becomes the evidence for whether `improv-wifi` should follow.

## Milestones

1. **A channel.** Board ID reading, both derivations, sticker generation, advertising a rotating handle, the `NNpsk0` handshake, and — the part that must be right first time — a stream layer over GATT where either end opens uni- or bidirectional streams, carrying JSON. Plus the web test page that drives all of it. Two things ride on it: a line of text from the browser that the device prints, proving the client-to-device direction, and the device's hostname and addresses, proving the other and standing in for the Iti's screen. Everything genuinely novel is in this milestone; what follows is operations on a pipe that already works.
2. **Wi-Fi, done properly.** Joining a network, and putting the device into access-point mode — the case Improv cannot express and the reason this protocol carries Wi-Fi at all.
3. **The rest of provisioning.** Device description, hostname, timezone, enrolment, logs, reboot, physical identification.
4. **A native app.** Android or iOS, once the protocol has stopped moving. This is also the earliest point a wake beacon can exist, since Web Bluetooth cannot advertise — and therefore the earliest the device can stop advertising continuously.

## Open questions

- [x] Whether a board with no strong source — no TPM, no burnt OTP, only its platform serials — is shippable at all, or is refused the way non-unique hardware is. **Answered: shippable, because devices already in the field have nothing else.** The derivation is therefore sized against that case, and those boards carry a weaker guarantee that the threat model states rather than hides. See "Boards with only their serial numbers".
- [x] argon2id parameters, measured on a Pi 5. **Answered: 2 GiB, one pass, two lanes**, costing 2.4 s on that board. Four lanes are slower than two there, and concurrency was verified not to change the digest. See "Measured parameters".
- [x] Whether address privacy is configurable through `bluer`, or needs BlueZ configuration alongside it — and whether re-registering an advertisement presents a fresh address, which is what keeps salt and address rotation in lockstep. **Answered: it is not configurable through `bluer` at all, and lockstep is unavailable — the daemon has no supported way to observe the controller rolling its address.** Address privacy is a documented host requirement; the device rolls the salt and the guarantee narrows to which device rather than same device. See "Tracking resistance".
- [x] Salt rotation period. **Answered: fifteen minutes.**
- [x] Does `yamux` build for `wasm32-unknown-unknown`? The web page depends on it, and the answer decides between adopting it and hand-rolling the stream layer. **Answered: yes, and it is adopted.** See "Streams, and why not QUIC".

## Testing notes

- The SMBIOS path is the testable one: a UEFI VM or CI runner has a system UUID where a Pi does not, so the whole derivation chain can be exercised without hardware.
- A board ID override for tests, and a defined behaviour when no board ID source exists at all.
- Measure the actual entropy of both board ID sources on hardware we ship: how many bits a Pi serial really carries on each model, and whether the SMBIOS UUIDs we see in the field are distinct rather than a vendor constant. The derivation parameters follow from the answer.
- Time the derivation on the slowest board in scope, since it sits on the boot path.
- Known-answer tests pinning both derivations, so a change to constants or parameters cannot silently invalidate every sticker already printed.
- Full handshake and RPC exercised over an in-memory duplex transport, with no BLE involved.
- Streams opened from each end, in both directions, concurrently, and while another is mid-transfer; a stream closed by one end leaves the others and the connection alive.
- Negative cases worth pinning down: wrong sticker secret, replayed advertisement, replayed handshake, truncated frames, a peer that authenticates and then sends garbage.
- Repeated failed handshakes must leave the device reachable by a legitimate operator, and must not be able to fill the disk with logs.
- Two advertisements from the same device across a salt roll must not be linkable without the sticker secret, and a scanner holding the secret must recognise both.
- Handle collision between two devices in range.
- Address reporting on a device with several interfaces, with IPv6, and with none up at all; loopback and link-local must not appear.
- An address change while a client is connected reaches it without the client asking.
- BlueZ-level testing against a virtual controller is possible but heavy; the protocol core should not need it.
- Deriving on a board without room for it: the daemon reports that there is not enough memory rather than being killed by the kernel. The failure is a kill by signal with nothing to catch, so a test that only checks the happy path will not see it.
- The advertisement and scan response together carry the service UUID, handle, salt, and version within the legacy 31-byte limit of each, with a 128-bit service UUID in the advertisement where scan filtering can see it.
- Both payloads carry the version marker, and a device presented with a version it does not support says so rather than failing to match silently.
- Precedence picks the same source on the device as it did in the generator, including the case where a source is present but reads as a sentinel and must be skipped.
- A board that gains a stronger source derives a different secret, and the device reports that its sticker no longer matches rather than advertising a handle nobody can match.
