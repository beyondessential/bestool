# Plan: `bestool canopy register`

Implement a new `bestool canopy register` subcommand that enrols a machine as a
Canopy server using Canopy's **operator-first** enrollment flow. This plan is
self-contained: it has no dependencies on Canopy internals beyond the HTTP
contract described here. (It originated from Canopy's server-enrollment work and
was moved here so it lives with the code that implements it.)

## Background: what changed in Canopy

Canopy moved from "device-first / ticket-pull" to "operator-first /
token-push" server enrollment.

- **Old flow (being removed):** bestool generated a `CanopyTicket` (via
  `bestool t meta-ticket`) carrying the machine's own public key; an operator
  pasted that ticket into Canopy to create the server.
- **New flow:** an operator creates the server *in Canopy first*, and Canopy
  hands them a **passphrase-encrypted enrollment ticket** plus a separate
  **4-word passphrase**. The operator runs `bestool canopy register` on the
  machine (feeding it the ticket and the passphrase), which decrypts the ticket
  and then claims the pre-created server over mTLS via a **challenge/response
  that proves the machine holds the private key** behind the certificate it
  presents.

Net effect for bestool:

1. **Add** `bestool canopy register` (ticket as a positional arg or stdin, passphrase
   prompted — see CLI shape).
2. **Remove** `bestool t meta-ticket` (the `CanopyTicket` producer) entirely —
   it has zero use once the Canopy change lands, so there's no deprecation
   window. Delete the command and any now-dead `CanopyTicket`-generation code it
   was the sole user of. Do not remove other `t` subcommands.

## The enrollment ticket

Canopy returns two things from `mint_enrollment`:

- `ticket` — base64 (standard) of an **age-encrypted** payload. The encryption
  is an age/scrypt passphrase profile: the exact same primitives bestool's
  `protect`/`reveal` already use (the `algae-cli` crate). Decrypt it with the
  passphrase to recover the JSON below.
- `passphrase` — a **4-word, lowercase, hyphen-separated** passphrase (e.g.
  `correct-horse-battery-staple`, ~52 bits from the EFF large wordlist) that
  decrypts the ticket. Shared **out-of-band** from the ticket.

The decrypted payload is this JSON:

```jsonc
{
  "v": "enroll-1",                              // version tag; reject anything else
  "api_url": "https://<canopy public server>",  // device-facing API origin
  "server_id": "<uuid>",
  "token": "<base64url of 32 random bytes>"      // the single-use enrollment secret
}
```

- Base64-decode the ticket (accept all base64 variants: standard, no-pad,
  url-safe, url-safe-no-pad — mirror Canopy's lenient decoding), then decrypt
  the resulting age bytestream with the passphrase. Reuse algae's
  `reveal`/`decrypt_stream` plus `PassphraseArgs` to prompt for the passphrase
  and decrypt.
- Validate `v == "enroll-1"`; fail clearly otherwise.
- `token` is a bearer secret — **never log it** (nor the decrypted payload). The
  ticket itself is encrypted, so it is safe to accept as an argv positional (the
  whole `bestool canopy register <ticket>` line is one copy-paste). The
  **passphrase** is the sensitive half: prompt for it interactively (algae
  `PassphraseArgs`) and **never** accept it on argv, so it can't land in shell
  history or `ps`/`/proc/<pid>/cmdline`.
- There is no `group_id` and no CA in the payload. `api_url` is served with a
  webPKI (Let's Encrypt) certificate, so verify the server's TLS against the
  system root store; do not pin a CA.

## What `register` does

1. **Read + decrypt + parse** the ticket: take it as a positional arg (or
   stdin/file), base64-decode it, prompt for the passphrase (algae `PassphraseArgs`),
   `decrypt_stream` the age bytestream, then parse the JSON and validate version
   and required fields (`api_url`, `server_id`, `token`).
2. **Establish the machine's mTLS identity.** Use the machine's existing
   bestool/Canopy client key+certificate if one is already provisioned; if not,
   generate a keypair and a self-signed client certificate (ECDSA — Canopy
   identifies devices by the certificate's SubjectPublicKeyInfo (SPKI) bytes, not
   by a CA chain, so self-signed is fine). Persist this identity in bestool's
   usual config/state location so subsequent Canopy calls reuse it. **You must
   retain the private key** — enrollment now requires signing a challenge with
   it. **Strongly prefer a hardware-bound key:** generate/store the private key
   in a TPM or secure element so it is non-exfiltratable — it cannot be copied to
   another machine without explicit operator action. This is the primary defence
   against key theft/cloning (Canopy verifies possession by signature, so a key
   that can't leave the box can't be impersonated elsewhere). Fall back to a
   file-stored key only where no secure element is available.
3. **Run the two-step enrollment handshake** (see HTTP contract), presenting the
   client certificate (mTLS) on both calls and verifying the server's TLS against
   system roots:
   - `begin` → receive a `nonce`.
   - Sign the transcript with the machine's private key, `complete` → bound.
4. **Persist the result**: store `server_id`, the returned `device_id`, and
   `api_url` so the agent knows it is enrolled and where to report.
5. **Report** success to the operator (server id, device id). On failure, print
   Canopy's error and exit non-zero. Note Canopy's register errors are
   intentionally **opaque** ("enrollment failed") and do not distinguish unknown
   server / bad token / bad signature — don't try to branch on the reason.

Make `register` **idempotent-friendly**: if the machine is already enrolled with
this identity (the token has been consumed), detect that and treat it as success
where possible rather than erroring.

## HTTP contract

Both calls are HTTPS with mTLS (present the machine client certificate) to
`api_url`. The endpoint is rate-limited per server.

### Step 1 — `POST {api_url}/servers/register/begin`

- **Body:** `{ "server_id": "<uuid>", "token": "<token>" }`
- **Success `200`:** `{ "nonce": "<base64 of 32 bytes>", "channel_binding_required": <bool> }`
- The token is **not** consumed here; the challenge nonce is short-lived
  (~minutes). If `channel_binding_required` is `true`, you must include the TLS
  exporter value in the signature (see below).

### Step 2 — `POST {api_url}/servers/register/complete`

- **Signature:** sign the transcript `nonce ‖ server_id ‖ SPKI [‖ EKM]` (the
  exact concatenation/encoding will be pinned by Canopy — coordinate the byte
  layout; e.g. raw nonce bytes ‖ server_id UUID bytes ‖ DER SPKI bytes) with the
  machine's private key, using the algorithm matching the cert key (ECDSA).
- **Channel binding (when `channel_binding_required`):** append the TLS exporter
  value (`EKM`) to the transcript before signing. Derive it from *this* TLS
  connection per RFC 9266 "tls-exporter": label `EXPORTER-Channel-Binding`, empty
  context, 32 bytes. The terminating proxy computes the same value and forwards
  it to Canopy, which checks your signature covers it — this binds the
  enrollment to the actual TLS session. (Requires a TLS stack that exposes RFC
  5705 exporters.)
- **Body:** `{ "server_id": "<uuid>", "nonce": "<from begin>", "signature": "<base64>" }`
- **Success `200`:** `{ "server_id": "<uuid>", "device_id": "<uuid>" }`
- **Errors:** RFC-7807-style problem JSON with a single opaque "enrollment
  failed" for all failure modes (unknown/archived server, invalid/expired/
  consumed token, bad/expired/used nonce, bad signature). Surface `title`/`detail`
  to the operator and exit non-zero.

Canopy verifies the signature against the SPKI of the cert presented on the
`complete` call (which must match the one presented at `begin`) — this is the
proof-of-possession. Only on success does Canopy consume the token and bind the
device.

## CLI shape

```
bestool canopy register <TICKET>   # ticket as a positional arg; prompts for the passphrase
bestool canopy register            # ticket from stdin if no positional given
```

- Accept the encrypted ticket as a **positional arg** (the operator copy-pastes
  the whole `bestool canopy register <ticket>` line from Canopy). Also accept it
  on **stdin** when no positional is given. The ticket is encrypted, so argv is
  fine.
- Prompt for the **passphrase** interactively via algae's `PassphraseArgs`
  (which also supports the usual non-interactive overrides). Never take it as an
  argv positional.
- Consider `--config <path>` to override where the mTLS identity/state is stored,
  consistent with other bestool subcommands.
- Place under a `canopy` subcommand group (new if it doesn't exist).

## Out of scope / notes

- Status/metric reporting after enrollment is unchanged — this command only
  performs enrollment. **Canopy is removing the device-driven `POST/PATCH/DELETE
  /servers` endpoints** (the operator now creates and edits the server record),
  so bestool must **stop self-creating/self-editing** the server record if it
  does so today. The public `GET /servers` mobile list is unaffected.
- Token lifetime is 7 days, single-use, reissuable from Canopy — bestool doesn't
  manage token lifecycle, it just presents whatever is in the decrypted payload.

## Security model: why encrypt the ticket

The `token` inside the ticket is the actual bearer secret for the PoP handshake;
the **public-server `register/begin|complete` endpoints and the token/PoP
handshake do not change** — bestool still presents the plaintext `token`.
Encryption only protects the ticket *in transit* between the operator's screen
and the target box, so the ticket can be pasted into chat/email/a config tool
without that channel alone being enough to enrol.

The residual brute-force risk is bounded by stacking:

- the 4-word passphrase is ~52 bits of entropy, and the age/scrypt KDF makes each
  guess deliberately expensive; and
- even a recovered `token` is single-use, expires in 7 days, is rate-limited
  per-server, and is **PoP-gated** (an attacker still has to sign the challenge
  with the machine's private key).

The benefit only holds if the **ticket and passphrase travel on different
channels** — shipping both through the same channel gives an observer everything
and defeats the point. The UI says as much.

- Canopy no longer returns a `central_public_key` (it was unused). If a real
  server-trust anchor is added later, this plan will be updated with its
  verification story.
