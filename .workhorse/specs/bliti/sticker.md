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

The QR code encodes the URL `https://bliti.tamanu.app/`, with the payload in its fragment.
A generic phone camera opens the page, so a device is usable without installing anything first, and the fragment is never sent to a server, so the secret stays on the device that scanned it.
A native application can claim the link, so scanning opens that application where it is installed.

The payload is rendered as unpadded base32 in the fragment, in the same characters as the rendering printed beneath the code.
A QR code spends fewer bits on digits and upper-case letters than on mixed-case text, so the longer base32 rendering produces a coarser code than a shorter mixed-case one would, and a coarser code is what a phone camera reads off an enclosure.
The URL is lower case, because a native application claims a link by matching the scheme and host literally.

A client that is already open reads the code with its own camera instead of following the link, as specified in [BLI-WEB](web-app.md).
Both paths yield the same payload: the URL carries it rather than forming part of it.

## Printing

A human-readable rendering of the payload is printed beneath the QR code, so a sticker whose code is scuffed or damaged remains usable.

## Generation

A sticker is generated from a board ID, read either from the board in front of the generator or from a list of board IDs gathered beforehand.

Whether such a list can be gathered before the boards are to hand depends on which source won the precedence in [BLI-BID](board-id.md).
A platform serial number can be known without the board present.
A board ID taken from a TPM Endorsement Key, or from written one-time-programmable memory, is readable only from the board itself, so stickers for those boards are generated with the board to hand.

The payload for a board is fixed, and no record of what was issued is kept or needed.
A damaged sticker is replaced by printing the same payload again, recovered from the code itself or from the human-readable rendering beneath it, and by deriving it from the board again where neither can be read.

Generation is refused where the board offers no usable source, as specified in [BLI-BID](board-id.md).
