---
id: BLI-STK
---

# Sticker

The sticker is printed and fixed to the outside of a device's enclosure, and carries the sticker secret of [BLI-KEY](key-schedule.md) in a QR code.
It is the credential: scanning it is what lets a client recognise and authenticate to that device.

## Payload

The QR payload carries the sticker secret and the version marker, and nothing else.

The board ID is not carried alongside the secret.
Putting it there would hand the board ID to anyone who photographs a sticker, which is the property the derivation exists to provide.

The payload is an `https://` URL with its content in the fragment.
A generic phone camera opens the page, so a device is usable without installing anything first, and the fragment is never sent to a server, so the secret stays on the device that scanned it.
A native application can claim the link, so scanning opens that application where it is installed.

## Printing

A human-readable rendering of the payload is printed beneath the QR code, so a sticker whose code is scuffed or damaged remains usable.

## Generation

A sticker is generated from a board ID, read either from the board in front of the generator or from a list of board IDs gathered beforehand.

Whether such a list can be gathered before the boards are to hand depends on which source won the precedence in [BLI-BID](board-id.md).
A platform serial number can be known without the board present.
A board ID taken from a TPM Endorsement Key, or from written one-time-programmable memory, is readable only from the board itself, so stickers for those boards are generated with the board to hand.

The same sticker is produced every time from the same board, so a damaged sticker is replaced by generating it again rather than by consulting a record of what was issued.

Generation is refused where the board offers no usable source, as specified in [BLI-BID](board-id.md).
