---
id: BLI
---

# bliti device provisioning

bliti provisions headless devices over Bluetooth Low Energy, anchored to a QR sticker printed on the device's enclosure.
A device advertises an opaque handle, and a client that has scanned that device's sticker — and only such a client — can recognise it among the advertisements it hears, authenticate to it, and open a two-way channel.

The sticker stands in for the button press or on-screen code that other provisioning protocols use to establish that the operator is physically present, because the devices bliti targets have neither a button nor a screen.
bliti is not an implementation of Improv Wi-Fi and does not interoperate with it.

bliti ships as its own daemon and its own sticker generator, separate from the other tools in this repository.

## The chain

Every value in the system descends from an identifier the board's own firmware provides, under constants that are public.

A **board ID** is read from firmware, as specified in [BLI-BID](board-id.md).
A **sticker secret** is derived from the board ID, and is the value printed in the QR code; an **advertised handle** is derived from the sticker secret, and is the value the device broadcasts.
Both derivations are specified in [BLI-KEY](key-schedule.md).

What is printed on the sticker, and how one is generated, is specified in [BLI-STK](sticker.md).

A client scans the sticker, recomputes the handle, and matches it against what it hears, as specified in [BLI-ADV](discovery.md).
Client and device then authenticate to each other and open a channel, as specified in [BLI-CHN](channel.md).

There is no fleet key and no authoritative per-device record.
The whole chain is reproducible from the board alone, at manufacture or at any time after, so a sticker can be reprinted from the device itself rather than from a record of what was issued.
Anything a device stores about its own identity is a cache it can rebuild.

## What the guarantees are

An operator holding a sticker can pick that device out of every device advertising nearby.

A listener on the BLE link learns neither the sticker secret nor the contents of a session, and cannot turn a recorded advertisement into a session.

Reading the QR code does not yield the board ID, and does not yield it even to someone who knows the derivation constants.
The board ID therefore appears in no QR payload, in no advertisement, and nowhere else reachable without access to the device.

An observer who has not scanned the sticker cannot tell which device an advertisement belongs to.

## Where the guarantees stop

These limits are properties of the design rather than gaps in it, and the system is described accurately by stating them.

Anyone who has had access to a device, or who otherwise knows its board ID, can derive its sticker secret and both impersonate it and connect to it.
The same is true of anyone holding a photograph of the sticker: the sticker is the credential.

An observer who has not scanned the sticker can still tell that some bliti device is present, because the service UUID is advertised in the clear so that clients can filter a scan on it.
Such an observer can also tell that two advertisements come from the same device whenever the adapter's address does not rotate, which is a property of how the host is configured rather than something bliti controls.
bliti claims only that such an observer cannot tell *which* device it is hearing.

Because the derivation constants are public, finding a device requires neither its sticker nor physical access to it, only its board ID — and board IDs can be searched for rather than known.
What stands against that search is the cost of the derivation and the size of the board ID's space, both specified in [BLI-KEY](key-schedule.md).
For boards whose only identifier is a short serial number, that margin is narrow, and those boards carry a weaker guarantee than boards with a hardware-backed identifier.

## How the layers sit

Each layer depends only on the one beneath it carrying bytes reliably and in order.

| layer | what it provides |
| --- | --- |
| BLE GATT | reliable, ordered bytes |
| framing | message boundaries across the negotiated attribute size |
| Noise `NNpsk0` | mutual authentication, encryption, a session key |
| stream multiplexing | either end opens unidirectional or bidirectional streams |
| JSON | application messages |

Replacing the bottom layer with another BLE transport changes nothing above it, and the choice can differ per client while the layers above stay identical.
