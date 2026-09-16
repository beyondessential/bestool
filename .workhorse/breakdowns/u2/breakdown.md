# bliti: deferred from the channel milestone

Work set aside while the channel milestone is built, held here so it survives the plan being cleaned up.
None of these is blocked by an open decision. The second depends on the first; the third stands alone.

## Advertise only in a window after power-on

A device advertises whenever it is running.
Advertising only for a window after power-on, then going quiet, narrows what a passive observer can see: a device with no button still has a power cable, and a power cycle needs someone standing at the box just as a button press does.
This is local policy about when the daemon advertises, with no bearing on the wire format, so adopting it later forecloses nothing.
The one client-side consequence is telling an operator to power-cycle a device that is not answering.

## Wake a quiet device with a targeted beacon

Once a device goes quiet, a wake beacon reopens its window without anyone unplugging it.
The beacon has to be targeted rather than broadcast: a wake that anyone can send and every device in range answers is a presence oracle someone could sweep a building with.
Deriving the wake signal from the sticker secret under a third constant closes that, and costs nothing in practice, since a client that has scanned a sticker already knows which device it wants.
This waits on a native application, because browsers cannot advertise at all, and nothing needs reserving in the wire format now.

## Debug mode from a removable volume

A device whose sticker no longer matches it cannot be reached at all, which strands a technician who has no way to access the device to obtain the replacement QR on the spot.
A file at a known filename at the root of a removable volume, read when the daemon starts, carries a random value used directly where the derived sticker secret would be, so discovery, matching and the handshake are unchanged and nothing needs reserving in the wire format.
The channel it opens exposes a restricted toolset: enough to read the real sticker secret for reprinting, plus diagnostics, and not the normal provisioning surface.
Also to consider: take the whole device out of service,so it can't be accidentally forgotten in this state.