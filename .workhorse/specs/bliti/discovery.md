---
id: BLI-ADV
---

# Discovery and matching

A device advertises continuously over BLE.
A client that holds a sticker recomputes the expected handle from it and matches that against the advertisements it hears, which is what lets an operator pick one device out of everything advertising nearby.

## What is advertised

The advertisement carries a service UUID identifying the device as speaking bliti, and a local name.
The scan response carries service data holding the advertised handle of [BLI-KEY](key-schedule.md), the current rotation salt, and the version marker.

The service UUID is a 128-bit UUID and appears in the advertisement rather than in the scan response, because filtering a scan by service UUID is the only filtering some client platforms offer and it is applied to the advertisement.
Client platforms present the advertisement and the scan response to an application as one set of advertised data.

Splitting the content this way is what fits it into the budget.
A legacy advertisement carries 31 bytes, of which the mandatory flags take three and a 128-bit service UUID takes eighteen, leaving ten.
The scan response provides a second 31 bytes, of which service data keyed by a 128-bit UUID takes eighteen before any content, leaving thirteen.

The handle is eight bytes, the salt four, and the version marker one, filling those thirteen exactly.

The local name carries the first four bytes of the handle rendered as eight hexadecimal characters, filling the ten bytes remaining in the advertisement.
A rendering of the whole handle does not fit.
This costs nothing, because the handle is not secret, and it gives a client platform that can only filter by name prefix something to filter on, and a human something to match by eye.

## Matching

A client scans, recomputes the handle from the sticker it holds together with whatever salt it observes, and compares.

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

## Address privacy

The privacy of the BLE address itself is a property of how the adapter is configured, and bliti neither sets it nor depends on it.

Where the adapter's address does not rotate, an observer can link a device's advertisements by address regardless of the salt, and the guarantee is the one stated in [BLI](overview.md): such an observer learns that a device is present and that it is the same device, but not which device it is.

## Advertising continuously

A device advertises whenever it is running, rather than only during a window after starting.

Anyone in range can therefore open a connection and begin a handshake that will fail.
Repeated failures leave the device reachable by a legitimate operator: there is no lockout, because someone in range could otherwise deny an operator their own device, which is worse than the attempts a lockout would prevent.
Failed attempts are not recorded so freely that someone in range can exhaust the device's storage by making them.
