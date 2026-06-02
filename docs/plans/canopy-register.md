# Plan: Canopy register — outstanding item

`bestool canopy register`, the machine-id-bound encrypted registration store
(with legacy `/etc/tamanu` migration), and `canopy export` / `canopy import`
have all shipped. One item from the original design remains deferred.

## TLS channel binding (RFC 9266 "tls-exporter")

`register` does not support `channel_binding_required`. When Canopy's
`register/begin` returns it `true`, the command errors out clearly.

To implement: append the TLS exporter value (label `EXPORTER-Channel-Binding`,
empty context, 32 bytes) to the signed proof-of-possession transcript. This
requires dropping the two handshake calls (`register/begin` +
`register/complete`) to a `rustls` + `hyper` stack where
`export_keying_material` is available — `reqwest` doesn't expose RFC 5705
exporters. The terminating proxy computes the same value and forwards it to
Canopy, which checks the signature covers it.
