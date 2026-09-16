---
id: BLI-BID
---

# Board ID

The board ID is the firmware-provided identifier that every other value in [BLI](overview.md) descends from.
Its job is to make the sticker secret reproducible: the same sticker can be regenerated from the board alone, with no per-device database to keep in sync.

The board ID is not a secret.
Any software on the device can read it.
Nothing depends on it staying hidden, only on it being expensive to search for, which is a property of its size and of the derivation in [BLI-KEY](key-schedule.md).

## Choosing a source

A board offers more than one candidate identifier, and the board ID is the strongest one present rather than a combination of them.

The precedence, strongest first, is the TPM Endorsement Key, then provisioned one-time-programmable memory, then the platform's serial numbers.
Precedence is evaluated by kind of source rather than by platform, so a board gains a stronger source simply by having the hardware for it, and no rule names a particular model.

The device and the sticker generator evaluate the same precedence against the same board and therefore select the same source, without either being told which kind of machine it is running on.

Combining sources is not done.
Each source in a combination would be a way for the board ID to change, and a board ID that changes orphans a sticker already fixed to an enclosure.

### TPM Endorsement Key

Where a TPM 2.0 is present, the board ID is the name of its Endorsement Key: the hash algorithm identifier followed by the digest of the key's public area.

The key is the one generated from the endorsement seed under the TCG low-range RSA 2048 template.
A TPM holds one Endorsement Key per algorithm, so naming the algorithm is part of the derivation rather than an implementation choice, and changing it re-derives every board ID taken under the old one.

The key is regenerated from the seed and the template rather than read from wherever provisioning software may have persisted it, because a persisted copy is not guaranteed to exist on a freshly imaged machine while the seed and the template always are.

This source is available both on machines with a firmware TPM and on boards fitted with a discrete TPM over SPI.

### Provisioned one-time-programmable memory

Where the board carries customer-programmable one-time-programmable memory that has been written, its contents are the board ID.

### Platform serial numbers

Otherwise the board ID is the platform's own serial number: the device-tree serial on Raspberry Pi hardware, or the SMBIOS system UUID on UEFI machines.

## Probing and reading

Establishing which sources a board has is separate from reading the value one of them holds.

Presence is cheap to establish: a device node exists or it does not, one-time-programmable memory reads as written or as blank, a serial is readable or absent.

Reading a value is not uniformly cheap.
A TPM Endorsement Key name is obtained by regenerating the key from the endorsement seed under the pinned template, which is a key generation inside the TPM rather than a file read, and it is paid every time the value is wanted.

Evaluating the precedence therefore means probing each source for presence, and reading a value only from the one that wins.

## Sources that carry no identity

A source can be present and still hold no identity.
A value reading as all zeros, as all ones, or as a known vendor constant is a placeholder whatever its nominal width, and deriving from it would give every board in the same position the same secret.

Such a source is skipped and precedence falls through to the next one.
Unwritten one-time-programmable memory is the ordinary case, reading as zeros on every unprogrammed board, and a board in that state derives from its serial number instead.

Reaching the end of the precedence with no usable source is a failure, reported as specified in [BLI](overview.md) rather than derived past.

## When the board ID changes

Fitting hardware that carries a stronger source changes which source wins, and so changes the board ID and every value below it.
A board that gains a TPM, or has its one-time-programmable memory written after its sticker was printed, no longer matches that sticker.

The platform serial number identifies the board across such a change.
Being the last tier of the precedence, it is present on every board in scope, so it is available whichever source wins, and it does not itself change when stronger hardware is fitted.

A board whose platform serial is unchanged, but whose strongest present source is stronger than the one it last derived from, has gained hardware, and the sticker on its enclosure is dead.
That is reported, rather than the device advertising a handle no client can match.
Recovering from it means printing a new sticker for that board.

A board whose platform serial differs is a different board, reached by moving a disk from one enclosure into another.
It derives from the board it now sits on, and matches the sticker already fixed to that enclosure, so this is not a fault and is not reported.

A board that offers no platform serial has no weaker source for a stronger one to supersede, so any change in its board ID is reported.

Because a board ID derived from a newly written source supersedes one derived from a serial number, writing that source is done before the sticker is derived and printed.

## Hardware in scope

bliti derives a board ID on physical machines.
Detecting virtualised or cloned machines is not part of the system.
