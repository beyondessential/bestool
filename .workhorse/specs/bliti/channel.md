---
id: BLI-CHN
---

# Authenticated channel

Once a client has matched a device by the handle in [BLI-ADV](discovery.md), the two authenticate to each other and open a channel carrying application messages.

## Authentication

Client and device run a Noise `NNpsk0` handshake with the sticker secret of [BLI-KEY](key-schedule.md) as the pre-shared key.

Both ends bring only ephemeral keys, and all authentication comes from the pre-shared secret.
This proves in both directions that each end holds the sticker secret, which is what "this is the device whose sticker I scanned" and "you scanned my sticker" both reduce to.
Neither a replayed advertisement nor a spoofed one yields a session, because an attacker cannot complete the handshake behind it.

The handshake produces a fresh session key and gives the session forward secrecy, so recovering a sticker secret later does not decrypt a recorded session.

A device's board ID is not verified directly, and cannot be: it is absent from the QR payload and the derivation does not run backwards.
Possession of the sticker secret is the proof, and it is equivalent, because deriving the secret requires the board ID.

The sticker secret is a full-width value rather than a short code a person types, which is why no password-authenticated key exchange is used: the resistance of the secret to guessing comes from the derivation in [BLI-KEY](key-schedule.md).

An eavesdropper who records a handshake can attempt the same offline search against the transcript as against an advertisement, and the same derivation cost and the same limits apply.

## Transport

The channel runs over GATT, using a characteristic written by the client and a characteristic the device notifies on.

Messages are framed and reassembled across the negotiated attribute size, so a message is not limited by it.

GATT is the transport every client platform can reach, including browsers, which have no other.

## The device is a peripheral only

The device acts only as a GATT server, and never as a GATT client against the client that connects to it.

A Bluetooth stack that resolves the connecting client's attributes in turn will meet one whose read requires an encrypted link, and ask to pair in order to read it.
No client this protocol serves can pair: a browser cannot drive pairing at all, and the sticker is what stands in for it.
The pairing attempt is therefore refused, and the device drops the link partway through a session that was otherwise working.

The device likewise never initiates pairing, and the channel never depends on the link being encrypted or the peer being bonded.
All of the protocol's authentication and secrecy comes from the handshake above.

Where the Bluetooth stack does this by default, turning it off is a prerequisite for running a device, alongside the stack itself.

## Streams

Above the handshake, either end opens streams, unidirectional or bidirectional, without coordinating identifiers with the other end and without asking permission, and several streams are in flight at once.

Stream identifiers are allocated from disjoint spaces per end, so the two ends cannot collide.

Closing one stream leaves the other streams and the connection itself alive.

This is what lets a device send without being asked, rather than only answering requests.
State that changes while a client is connected is sent as it happens rather than waiting to be polled for.

## Messages

Application messages are JSON.

The volumes involved are small, every client platform reads JSON without a library, and a conversation can be read directly while developing.

A device that receives a message it does not understand says so, rather than closing the channel.
