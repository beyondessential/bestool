---
id: BLI-KEY
---

# Key schedule

Two derivations take the board ID of [BLI-BID](board-id.md) to the values [BLI](overview.md) depends on: the sticker secret printed in the QR code, and the advertised handle broadcast over BLE.

Both derivation constants are public.
They are compiled into the device, the sticker generator, and every client, and publishing them weakens nothing, because neither derivation runs backwards.
What the constants provide is domain separation, so that a value from one step is not a valid value at another.

## Sticker secret

The sticker secret is derived from the board ID with argon2id, under a fixed constant, producing a 32-byte value.

The derivation uses 2 GiB of memory, a single pass, and two lanes.
These parameters are part of the derivation rather than a tuning choice: changing any of them produces a different secret and orphans every sticker already printed under the old ones.

Whether an implementation computes the lanes concurrently or in sequence does not affect the result, so implementations are free to choose either.

The parameters are memory-heavy rather than pass-heavy because the memory parameter bounds how many guesses an attacker can run at once, while passes only make each guess longer.
They are chosen so that the derivation is affordable on the slowest board in scope while remaining costly in bulk.

### What is derived from

The argon2id password is a source tag byte followed by the raw bytes of the source value, most significant first.
The argon2id salt is the fixed constant.

The raw bytes are used, never a text rendering of them.
A Raspberry Pi serial is read as characters and an SMBIOS system UUID as a dashed string, and deriving from those characters rather than from the eight and sixteen bytes they denote produces a different secret, permanently, once a sticker carrying it has been printed.

The tag identifies which kind of source the value came from, so a value that is byte-identical across two kinds of source still derives differently.
Lengths are fixed per kind of source, so the tag leaves the input unambiguous without a length prefix.

### What the cost achieves

The derivation cost is what stands between a public constant and an attacker enumerating the board ID space offline, computing every sticker secret and handle and matching them against advertisements.

For a board whose board ID comes from a TPM Endorsement Key or from written one-time-programmable memory, the space is large enough that no derivation cost is load-bearing, and the cost is depth rather than the thing holding the scheme up.

For a board identified by a serial number, the cost is what the guarantee rests on, and it does not make every such board safe.
A Raspberry Pi 4 or 5 serial number occupies its full width and is out of reach.
A serial number that collapses to a short value, as on earlier Raspberry Pi boards, and a SMBIOS system UUID that merely reformats a vendor's service tag, are both small enough to be searched by an adversary willing to spend on it, and no parameters tolerable on a provisioning path change that.
Those boards carry the weaker guarantee described in [BLI](overview.md).

### Deriving on the device

The device derives its own sticker secret, which is what makes the chain reproducible from the board everywhere rather than only on large machines.

The derivation is paid once and cached, rather than repeated at each start.

The cache holds the derived secret together with the board ID it was derived from, the platform serial of the board it was derived on, and which kind of source won the precedence.
Establishing whether the cache still holds is a comparison of those values against the board, not a rederivation: the memory-hard derivation runs only where the cache is absent or the comparison fails.

On start the device reads the platform serial and probes which kinds of source are present, both cheap as specified in [BLI-BID](board-id.md).
Where the serial and the strongest kind present both match the cache, the cached secret stands, and no source value is read and no derivation runs.
Where the strongest kind present is stronger than the cached one, the board has gained hardware and its sticker is dead, which is reported rather than derived past.
Where the serial differs, the device is on another board: it evaluates the precedence, reads the winning source, and derives.

The derivation needs its full memory parameter available at once, and a device without room for it is killed by the operating system rather than told that the allocation failed.
The device therefore establishes that there is room before beginning, and reports that there is not, rather than terminating part-way through with nothing reported.

## Advertised handle

The advertised handle is derived from the sticker secret and the current rotation salt with a fast keyed hash, under a second fixed constant, and truncated to eight bytes for advertising.

This derivation is deliberately cheap.
A client recomputes it for every advertisement it hears, against every sticker it holds, so a memory-hard function here would be felt during scanning.

Eight bytes makes a collision between two devices at one site implausible, and fits the advertising budget in [BLI-ADV](discovery.md).

## Versioning

Everything a sticker depends on is versioned together under a single marker, carried both in the QR payload of [BLI-STK](sticker.md) and in the advertisement of [BLI-ADV](discovery.md).

The marker covers the derivation constants, the argon2id parameters, the source precedence and the encoding above, the pinned Endorsement Key template, and the handle length.
Any of these changing is a new version, because any of them changing changes the secret.

Nothing else is a new version.
A change that leaves the secret identical does not move the marker, because moving it orphans every sticker already fixed to an enclosure.
The URL the QR code is carried in, and the human-readable rendering printed beneath it, are carriers rather than payload, and changing either leaves the marker where it is.

A client holds the sticker, reads its version, and derives once under that version.

A device holds no sticker and cannot know which version was printed for it, so supporting more than one means deriving under each and advertising under each.
A device advertises one version at a time.
