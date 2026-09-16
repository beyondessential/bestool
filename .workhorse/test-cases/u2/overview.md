# bliti test cases

Scenarios verifying the specs under `.workhorse/specs/bliti/`.
An unticked case is coverage still owed, not an optional extra.

## Board ID

- [ ] The SMBIOS path derives end to end without Raspberry Pi hardware, on a UEFI machine or CI runner that has a system UUID (verifies spec: BLI-BID)
- [x] A board ID override is available for tests, so the chain can be exercised against known inputs (verifies spec: BLI-BID)
- [x] Precedence selects the same source on the device as in the generator, for each combination of sources present (verifies spec: BLI-BID) — also exercised against two real boards: a UEFI machine selects its TPM over its SMBIOS UUID, the prototype its serial over unwritten OTP
- [x] A source present but reading as all zeros, all ones, or a known vendor constant is skipped, and precedence falls through to the next (verifies spec: BLI-BID)
- [x] Unwritten one-time-programmable memory falls through to the serial number rather than deriving from zeros (verifies spec: BLI-BID) — also confirmed on the prototype, whose customer OTP reads as 32 zeros
- [x] Reaching the end of the precedence with no usable source fails, rather than deriving from a placeholder (verifies spec: BLI-BID) — the core returns a no-usable-source failure; surfacing it on standard error is daemon work
- [x] The Endorsement Key is regenerated from the seed and the pinned template rather than read from a persistent handle, and gives the same value on a machine where no handle has been persisted (verifies spec: BLI-BID) — matches `tpm2_createek` byte for byte on an Intel firmware TPM
- [x] Probing for presence reads no source value, so a board carrying a TPM is probed without a key generation inside it (verifies spec: BLI-BID)

## Identity changes

- [x] A board whose platform serial is unchanged, but whose strongest present source is now stronger than the one it last derived from, reports that its sticker is dead rather than advertising a handle nobody can match (verifies spec: BLI-BID)
- [x] A disk moved into another enclosure derives from the board it now sits on, matches the sticker already fixed to that enclosure, and reports nothing (verifies spec: BLI-BID)
- [x] A board offering no platform serial reports any change in its board ID (verifies spec: BLI-BID)

## Key schedule

- [x] Known-answer tests pin both derivations, so a change to constants or parameters cannot silently invalidate every sticker already printed (verifies spec: BLI-KEY)
- [x] The derivation input is a source tag byte followed by the raw bytes of the source value, and a serial derived from its characters gives a different secret from the bytes those characters denote (verifies spec: BLI-KEY)
- [x] Two sources holding byte-identical values derive different secrets, because their tags differ (verifies spec: BLI-KEY)
- [x] The sticker secret is identical whether argon2 lanes are computed concurrently or in sequence (verifies spec: BLI-KEY) — one known-answer vector holds across both settings and across x86-64 and aarch64
- [x] The derivation is timed on the slowest board in scope, since it sits on the provisioning path (verifies spec: BLI-KEY) — 2.2 s concurrent, 3.1 s sequential, on the Raspberry Pi 5 prototype
- [x] A device without room for the derivation reports insufficient memory rather than being killed part-way through with nothing reported (verifies spec: BLI-KEY)
- [x] A start where the platform serial and the strongest kind of source present both match the cache runs no derivation and reads no source value (verifies spec: BLI-KEY)
- [x] The cache is rebuilt where it is absent, and where the comparison against the board fails (verifies spec: BLI-KEY)
- [x] The handle is eight bytes (verifies spec: BLI-KEY)

## Entropy of the sources

- [ ] The real entropy of each board ID source is measured on hardware we ship: how many bits a serial number carries on each model, and whether the SMBIOS UUIDs seen in the field are distinct rather than a vendor constant. The derivation's adequacy for a given board follows from the answer (verifies spec: BLI-KEY)

## Discovery

- [x] The advertisement carries the service UUID, and the local name carries the eight-byte handle, four-byte salt and one-byte version, within the legacy budgets (verifies spec: BLI-ADV) — the budget is a compile-time assertion, and the advertisement registers and is heard on a legacy-only controller
- [x] The local name carries the handle, salt and version as 21 characters of base32, and changes when the salt rolls (verifies spec: BLI-ADV)
- [x] Both the QR payload and the advertisement carry the version marker (verifies spec: BLI-ADV)
- [ ] A client hearing a version it does not hold reports a device present at an unsupported version, distinctly from hearing nothing at all (verifies spec: BLI-ADV)
- [x] Two advertisements from one device across a salt change are not linkable without the sticker secret, and a client holding the secret recognises both (verifies spec: BLI-ADV)
- [ ] A handle collision between two devices in range is handled rather than silently picking one (verifies spec: BLI-ADV)
- [ ] Repeated failed handshakes leave the device reachable by a legitimate operator, and cannot fill its storage with records of them (verifies spec: BLI-ADV)

## Channel

- [x] The full handshake and a message exchange run over an in-memory transport, with no BLE involved (verifies spec: BLI-CHN)
- [x] Streams open from each end, in both directions, concurrently, and while another is mid-transfer (verifies spec: BLI-CHN)
- [x] A stream closed by one end leaves the other streams and the connection alive (verifies spec: BLI-CHN)
- [x] A message a device does not understand is reported rather than closing the channel (verifies spec: BLI-CHN)
- [ ] Negative cases are pinned: wrong sticker secret, replayed advertisement, replayed handshake, truncated frames, and a peer that authenticates and then sends garbage (verifies spec: BLI-CHN) — wrong secret, transport-message replay, and tamper are pinned; replayed advertisement and replayed handshake await the daemon

## Web application

- [ ] Following the link opens the application with the payload in the fragment, and the fragment is not sent to a server (verifies spec: BLI-WEB)
- [ ] Capturing the code with the camera yields the same payload as following the link, and the application treats the two identically (verifies spec: BLI-WEB)
- [ ] A payload the application cannot parse, and one carrying a version it does not support, are each reported as what they are (verifies spec: BLI-WEB)
- [ ] A browser chooser is filtered by local name, so the device whose sticker was read is the one presented (verifies spec: BLI-WEB)
- [ ] The application runs no memory-hard derivation (verifies spec: BLI-WEB)
- [ ] The application runs from an https origin and from localhost, and reports the missing capability where the context is not secure (verifies spec: BLI-WEB)

## Channel demonstration

- [ ] Addresses are reported on a device with several interfaces, with addresses of more than one family, and with none up at all — the first two were seen over BLE on the prototype (five addresses across two interfaces, both families); a device with nothing up is still to try
- [x] Loopback and link-local addresses do not appear, and each reported address carries its interface
- [ ] An address changing while a client is connected reaches that client without it asking
- [ ] Text sent from a client appears on the device's standard output, and in the system log once it runs as a service

## Sticker

- [x] The same board produces the same payload every time (verifies spec: BLI-STK)
- [x] A damaged sticker is replaced by reprinting the payload recovered from the code or from the rendering beneath it, with no record of what was issued consulted (verifies spec: BLI-STK)
- [x] The QR payload contains the sticker secret and version and does not contain the board ID (verifies spec: BLI-STK)
- [x] The human-readable rendering beneath the code reproduces the payload, and is usable when the code itself cannot be scanned (verifies spec: BLI-STK)
- [x] Generation is refused for a board with no usable source (verifies spec: BLI-STK) — and refused too where a source is present but unreadable, which would otherwise fall through to a weaker one

## End to end

- [ ] The web application completes scan, match, handshake, and both directions against a real device
- [x] A command-line client completes scan, match, handshake, and the device-to-client direction against a real device over BLE — reached once; see the plan for what is not yet repeatable
- [ ] A command-line client reaches a device on a second and subsequent connection, rather than only the first of a session
- [ ] A phone camera opens the sticker URL without anything installed first, and the fragment is not sent to a server

## Notes

BlueZ-level testing against a virtual controller is possible but heavy, and the protocol core should not need it.

The TPM seed's stability across a BIOS-level TPM reset, a firmware TPM being disabled and re-enabled, and a firmware update is untested and needs establishing on the hardware to be shipped, before any sticker is printed for it. A change there orphans every sticker on that model at once.
