---
id: BLI-ADV
---

# Discovery and matching

A device advertises continuously over BLE.
A client that holds a sticker recomputes the expected handle from it and matches that against the advertisements it hears, which is what lets an operator pick one device out of everything advertising nearby.

## What is advertised

The advertisement carries a service UUID identifying the device as speaking bliti.
The local name carries the advertised handle of [BLI-KEY](key-schedule.md), the current rotation salt, and the version marker.

The service UUID is a 128-bit UUID and appears in the advertisement rather than the scan response, because filtering a scan by service UUID is the only filtering some client platforms offer and it is applied to the advertisement.

The payload is carried in the local name rather than in service data, because a device cannot choose where each element is placed.
A controller that does only legacy advertising offers 31 bytes for the advertisement and 31 for the scan response, and the host decides which element goes in which.
The mandatory flags take three bytes and a 128-bit service UUID eighteen, so 21 of the advertisement's 31 are already spent, and service data keyed by that same UUID needs 31 of its own before it will fit anywhere.
Carrying the payload as a local name costs two bytes of element header rather than eighteen of repeated UUID, and a local name is the one element a host will place in the scan response, so the whole advertisement fits a legacy controller.

The payload is 13 bytes: an eight-byte handle, a four-byte salt, and a one-byte version marker.
It is rendered as 21 characters of unpadded base32, which is what the local name holds.

Eight bytes of handle makes a collision between two devices at one site implausible.

A client platform that can only filter by name prefix has the rendering to filter on, and the handle is not secret, so carrying it in the clear costs nothing.

## Matching

A client scans, reads the local name of each device advertising the service UUID, decodes it, recomputes the handle from the sticker it holds together with the salt it observes, and compares.

A local name that is not a bliti payload belongs to a device that is not one, and is passed over.

Matching is by payload rather than by device address, so a client that is never shown the peer's address can still identify the device, and a device whose address rotates is still recognised.

The cost to a client is one fast hash per advertisement heard per sticker held.

A client reads the advertised version marker before recomputing.
Where it differs from the version of the sticker the client holds, the client reports a device present at a version it does not support.
No two versions produce a matching handle, so reading the marker is what separates that from a device the client cannot hear at all.

## Rotation

The rotation salt is a short random value advertised in the clear, and it changes every fifteen minutes.

Rotating it is what stops the handle being a fixed beacon: without the salt changing, a passive observer could follow a device by its handle alone even though the handle reveals nothing about which device it is.
An observer who has not scanned the sticker cannot link two advertisements across a salt change, while a client that holds the sticker recognises the device across it by recomputing.

Rotating the salt means re-registering the advertisement, and a client recomputes against whatever salt it observes, so nothing a client does depends on the rotation period.
The local name changes with the handle, so what a client filters on changes at the same time.

## Address privacy

The privacy of the BLE address itself is a property of how the adapter is configured, and bliti neither sets it nor depends on it.

Where the adapter's address does not rotate, an observer can link a device's advertisements by address regardless of the salt, and the guarantee is the one stated in [BLI](overview.md): such an observer learns that a device is present and that it is the same device, but not which device it is.

## Advertising continuously

A device advertises whenever it is running, rather than only during a window after starting.

Anyone in range can therefore open a connection and begin a handshake that will fail.
Repeated failures leave the device reachable by a legitimate operator: there is no lockout, because someone in range could otherwise deny an operator their own device, which is worse than the attempts a lockout would prevent.
Failed attempts are not recorded so freely that someone in range can exhaust the device's storage by making them.
