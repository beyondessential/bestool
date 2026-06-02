# Plan: Canopy registration store + export/import

`bestool canopy register` shipped (operator-first enrollment: decrypt the
passphrase-encrypted ticket, then claim the pre-created server over mTLS via a
proof-of-possession challenge/response). This plan covers the next iteration:
consolidate all enrollment state into a single machine-bound encrypted file,
add `canopy export` / `canopy import`, and point the reporting path at the new
store. The one remaining deferred item (TLS channel binding) is at the bottom.

## Goal

Replace the scattered plaintext files (`/etc/tamanu/server-id`,
`/etc/tamanu/device-key.pem`, `/etc/tamanu/canopy-registration.json`) with a
single encrypted file:

- Linux: `/etc/bestool/canopy-registration`
- Windows: `C:\bestool\canopy-registration`

The file is age/scrypt-encrypted under a passphrase **derived from the host's
machine id**, so a cloned disk ("golden image") cannot use the registration on
a different machine, and the private key is no longer at rest in plaintext.
This is a weaker, immediately-shippable stand-in for a TPM-bound key: the only
thing that changes for TPM later is *where the unlock key comes from* — decrypt
via the TPM instead of the machine id — without touching the file format or any
consumer.

## Encryption scheme

One format everywhere (age/scrypt, the same primitives the enrollment ticket
and `protect`/`reveal` already use). Only the passphrase source differs:

- **Local file:** passphrase = `blake3::derive_key("bestool canopy-registration
  v1", machine_id_bytes)`, encoded to a string. Never prompted.
- **Export blob:** passphrase = a freshly generated **random-char** passphrase
  (base64-url of 16 random bytes, ~128 bits — no wordlist, no binary bloat),
  printed for the operator to carry out of band.

Machine id source:

- Linux: `/etc/machine-id`, falling back to `/var/lib/dbus/machine-id`.
- Windows: `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid` (via `winreg`).
- The resolved base directory and (for tests) behaviour are overridable via
  `BESTOOL_CANOPY_DIR`.

## Registration model

```jsonc
{
  "v": "registration-1",
  "server_id":  "<uuid>",     // optional: absent on never-enrolled hosts
  "device_key": "<pem>",      // optional, SECRET: the mTLS private key
  "device_id":  "<uuid>",     // optional: set once enrolled
  "api_url":    "https://..." // optional: defaults to DEFAULT_CANOPY_URL
}
```

`register` populates all fields. Migration fills whatever the legacy files
provide. No `Debug` that prints `device_key`.

## The store (in `bestool-canopy`)

A new `registration` module in the canopy crate (already depended on by alertd,
doctor, tags, and the canopy commands):

- `default_dir()` / `registration_path()` — OS defaults, `BESTOOL_CANOPY_DIR`
  override.
- `load() -> Result<Option<Registration>>` — read+decrypt the default file; if
  absent, run legacy migration (below). `load_from(dir)` reads a specific dir
  with **no** legacy migration (for `--config` and tests).
- `store(&Registration)` / `store_in(dir, &Registration)` — encrypt under the
  machine-id passphrase, atomic write, `0o600`, creating the dir as needed.
- `encrypt_with_passphrase(&Registration, &SecretString)` /
  `decrypt_with_passphrase(&[u8], &SecretString)` — for export/import.

Uses the `age` crate directly (sync, in-memory) so the daemon doesn't pull in
pinentry/dialoguer. Adds `age`, `blake3`, and (Windows) `winreg` to the canopy
crate.

### Legacy migration (with verified deletion)

When the default file is absent, `load()` migrates from the old plaintext files:

1. Read `/etc/tamanu/server-id` and `/etc/tamanu/device-key.pem` (either may be
   absent). If neither exists, return `None`.
2. Build a `Registration` from what's present (`device_id` = none, `api_url` =
   none) and `store()` it (encrypt + atomic write to the new path).
3. **Re-read the new file from scratch** (decrypt) and verify it round-trips to
   the same `server_id`/`device_key`.
4. Only on a verified round-trip, delete the old plaintext files.
5. Any write/verify/delete failure is non-fatal: log a warning, return the
   in-memory `Registration` so this run still works, and leave the old files in
   place to retry next run. Legacy migration is skipped entirely when
   `BESTOOL_CANOPY_DIR` is set.

## Reporting path (read side)

The encrypted registration becomes the source of truth. A new tamanu feature
`canopy-registration` (adds `dep:bestool-canopy`) gates a registration-first
branch in `server_info`:

- `fetch_device_key_with` — registration `device_key` first, then the existing
  file/DB fallback.
- `get_or_create_server_id` — registration `server_id` first, then the existing
  file/DB path.
- The local `read_device_key()` helpers in `tamanu psql` and `tamanu tags` go
  through the same registration-aware resolver.

bestool enables `bestool-tamanu/canopy-registration` from every feature that
reads these (alertd, doctor, alerts, tags, psql) and from the canopy commands.
Reporting keeps posting to `DEFAULT_CANOPY_URL`; consuming the stored `api_url`
is out of scope here.

## Commands

- `bestool canopy register` — reuse the device key from an existing
  registration if present, else generate one; on success `store()` the full
  registration (no standalone files). `--config <DIR>` targets a specific dir.
- `bestool canopy export [--config <DIR>]` — `load()` the registration, generate
  a random passphrase, print the passphrase and the base64 of the
  passphrase-encrypted blob (clearly labelled, with the "send on separate
  channels" note). Errors if there's nothing to export.
- `bestool canopy import [<BLOB>] [--config <DIR>]` — blob as positional or
  stdin; prompt for the passphrase (algae `PassphraseArgs`); decrypt, then
  re-`store()` under this machine's id. Lenient base64 decode.

Features: `canopy-register`, `canopy-export`, `canopy-import`, all under the
`canopy` aggregate and all requiring `__canopy`.

## Deferred: TLS channel binding (RFC 9266 "tls-exporter")

`register` does not yet support `channel_binding_required`. When Canopy's
`register/begin` returns it `true`, the command errors out clearly. Supporting
it means appending the TLS exporter value (label `EXPORTER-Channel-Binding`,
empty context, 32 bytes) to the signed transcript, which requires dropping the
two handshake calls to a `rustls` + `hyper` stack where `export_keying_material`
is available (reqwest doesn't expose RFC 5705 exporters).
