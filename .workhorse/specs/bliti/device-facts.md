---
id: BLI-DEV
---

# Device facts over the channel

Over the channel of [BLI-CHN](channel.md), a device reports facts about itself that an operator standing in front of it would otherwise need a screen to read.

Carrying these over the channel is what makes a display optional on a device that never needed one for its own sake.
It is also more private than a display: anything drawn on a panel is readable by anyone walking past, whereas these facts require having scanned the sticker.

## Addresses and hostname

A device reports its hostname and its network addresses.

Every address that is up and global is reported, each with the interface it belongs to.
Loopback and link-local addresses are not reported, being noise in any presentation of them.

Addresses of every family are reported, and the number of them is not truncated.
Deciding which of them are worth showing, and how many, is the client's to make: the limits that apply to a small physical display are properties of that display rather than of the information.

Addresses are sent when they change while a client is connected, without the client asking, because an address appearing as a network comes up is exactly what an operator provisioning a device is waiting for.

## Echoing text

A device prints text sent to it by a client.

The text is written to the device's standard output, which reaches both a developer running the daemon directly and the system log once it runs as a service.
