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

A sticker is generated from a board ID, either by reading it from the board itself or from a manifest of board IDs.

The same sticker is produced every time from the same board, so a damaged sticker is replaced by generating it again rather than by consulting a record of what was issued.

Generation is refused where the board offers no usable source, as specified in [BLI-BID](board-id.md).
