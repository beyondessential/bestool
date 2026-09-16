# bliti: deferred from the channel milestone

Work set aside while the channel milestone is built, held here so it survives the plan being cleaned up.
Neither entry is blocked by an open decision, and the second depends on the first.

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
