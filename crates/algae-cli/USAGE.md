# Command-Line Help for `algae-cli`

This document contains the help content for the `algae-cli` command-line program.

**Command Overview:**

* [`algae-cli`↴](#algae-cli)
* [`algae-cli decrypt`↴](#algae-cli-decrypt)
* [`algae-cli encrypt`↴](#algae-cli-encrypt)
* [`algae-cli keygen`↴](#algae-cli-keygen)
* [`algae-cli protect`↴](#algae-cli-protect)
* [`algae-cli reveal`↴](#algae-cli-reveal)

## `algae-cli`

Simple, user-friendly, encryption commands.

Algae is a simplified profile of the excellent [age](https://age-encryption.org/v1) format.

It implements five functions for the most common operations, and tries to be as obvious and hard-to-misuse as possible, without being prohibitively hard to use, and while retaining forward-compatibility with age (all algae products can be used with age, but not all age products may be used with algae).

To start with, generate a keypair with `algae keygen`. This will generate two files: `identity.txt.age`, a passphrase-protected keypair, and `identity.pub`, the public key in plain.

To encrypt a file, use `algae encrypt -k identity.pub filename`. As this uses the public key, it doesn't require a passphrase. The encrypted file is written to `filename.age`. To decrypt it, use `algae decrypt -k identity.txt.age filename.age`. As this uses the secret key, it will prompt for its passphrase. The decoded file is written back to `filename` (i.e. without the `.age` suffix).

To obtain a plaintext `identity.txt` (i.e. to remove the passphrase), use `algae reveal identity.txt.age`. To add a new passphrase on a plaintext identity, use `algae protect identity.txt`. These commands are not special to identity files: you can `protect` (encrypt) and `reveal` (decrypt) arbitrary files with a passphrase.

Every command has a short help (`-h`), which is useful to recall the name of options, and a long help (`--help`), which contains more details and guide-level information.

**Usage:** `algae-cli <COMMAND>`



###### **Subcommands:**

* `decrypt` — Decrypt a file using a secret key or an identity
* `encrypt` — Encrypt a file using a public key or an identity
* `keygen` — Generate an identity (key pair) to encrypt and decrypt files
* `protect` — Encrypt a file using a passphrase
* `reveal` — Decrypt a file using a passphrase



## `algae-cli decrypt`

Decrypt a file using a secret key or an identity.

Either of `--key-path` or `--key` must be provided.

For symmetric cryptography (using a passphrase), see `protect`/`reveal`.

**Usage:** `algae-cli decrypt [OPTIONS] <INPUT>`

###### **Arguments:**

* `<INPUT>` — File to be decrypted

###### **Options:**

* `-o`, `--output <OUTPUT>` — Path or filename to write the decrypted file to.

   If the input file has a `.age` extension, this can be automatically derived (by removing the `.age`). Otherwise, this option is required.
* `-k`, `--key-path <KEY_PATH>` — Path to the key or identity file to use for encrypting/decrypting.

   The file can either be:
   - an identity file, which contains both a public and secret key, in age format;
   - a passphrase-protected identity file;
   - a secret key in Bech32 encoding (starts with `AGE-SECRET-KEY`);
   - when encrypting, a public key in Bech32 encoding (starts with `age`).

   When encrypting and provided with a secret key, the corresponding public key
   will be derived first; there is no way to encrypt with a secret key such that
   a file is decodable with the public key.

   ## Examples

   An identity file:

   ```identity.txt
   # created: 2024-12-20T05:36:10.267871872+00:00
   # public key: age1c3jdepjm05aey2dq9dgkfn4utj9a776zwqzqcar3879smuh04ysqttvmyd
   AGE-SECRET-KEY-1N84CR29PJTUQA22ALHP4YDL5ZFMXPW5GVETVY3UK58ZD6NPNPDLS4MCZFS
   ```

   An passphrase-protected identity file:

   ```identity.txt.age
   age-encryption.org/v1
   -> scrypt BIsqC5QmFKsr4IJmVyHovQ 20
   GKscLTw0+n/z+vktrgcoW5eCh0qCfTkFnbTFLrhvXrI
   --- rFMmV2H+FgP27oaLC6SHQOLy5d5DPGSp2pktFo/AOh8
   U�`OZ�rGЕ~N}Ͷ
   MbE/2m��`aQfl&$QCx
   n:T?#�k!_�ΉIa�Y|�}j[頙߄)JJ{څ1y}cܪB���7�
   ```

   A public key file:

   ```identity.pub
   age1c3jdepjm05aey2dq9dgkfn4utj9a776zwqzqcar3879smuh04ysqttvmyd
   ```

   A secret key file:

   ```identity.key
   AGE-SECRET-KEY-1N84CR29PJTUQA22ALHP4YDL5ZFMXPW5GVETVY3UK58ZD6NPNPDLS4MCZFS
   ```
* `-K <KEY>` — The key to use for encrypting/decrypting as a string.

   This does not support the age identity format, only single keys.

   When encrypting and provided with a secret key, the corresponding public key
   will be derived first; there is no way to encrypt with a secret key such that
   a file is decodable with the public key.

   There is no support for password-protected secret keys.

   ## Examples

   With a public key:

   ```console
   --key age1c3jdepjm05aey2dq9dgkfn4utj9a776zwqzqcar3879smuh04ysqttvmyd
   ```

   With a secret key:

   ```console
   --key AGE-SECRET-KEY-1N84CR29PJTUQA22ALHP4YDL5ZFMXPW5GVETVY3UK58ZD6NPNPDLS4MCZFS
   ```
* `-P`, `--passphrase-path <PASSPHRASE_PATH>` — Path to a file containing a passphrase.

   The contents of the file will be trimmed of whitespace.
* `--insecure-passphrase <INSECURE_PASSPHRASE>` — A passphrase as a string.

   This is extremely insecure, only use when there is no other option. When on an interactive terminal, make sure to wipe this command line from your history, or better yet not record it in the first place (in Bash you often can do that by prepending a space to your command).



## `algae-cli encrypt`

Encrypt a file using a public key or an identity.

Either of `--key-path` or `--key` must be provided.

For symmetric cryptography (using a passphrase), see `protect`/`reveal`.

**Usage:** `algae-cli encrypt [OPTIONS] <INPUT>`

###### **Arguments:**

* `<INPUT>` — File to be encrypted

###### **Options:**

* `-o`, `--output <OUTPUT>` — Path or filename to write the encrypted file to.

   By default this is the input file, with `.age` appended.
* `--rm` — Delete input file after encrypting
* `-k`, `--key-path <KEY_PATH>` — Path to the key or identity file to use for encrypting/decrypting.

   The file can either be:
   - an identity file, which contains both a public and secret key, in age format;
   - a passphrase-protected identity file;
   - a secret key in Bech32 encoding (starts with `AGE-SECRET-KEY`);
   - when encrypting, a public key in Bech32 encoding (starts with `age`).

   When encrypting and provided with a secret key, the corresponding public key
   will be derived first; there is no way to encrypt with a secret key such that
   a file is decodable with the public key.

   ## Examples

   An identity file:

   ```identity.txt
   # created: 2024-12-20T05:36:10.267871872+00:00
   # public key: age1c3jdepjm05aey2dq9dgkfn4utj9a776zwqzqcar3879smuh04ysqttvmyd
   AGE-SECRET-KEY-1N84CR29PJTUQA22ALHP4YDL5ZFMXPW5GVETVY3UK58ZD6NPNPDLS4MCZFS
   ```

   An passphrase-protected identity file:

   ```identity.txt.age
   age-encryption.org/v1
   -> scrypt BIsqC5QmFKsr4IJmVyHovQ 20
   GKscLTw0+n/z+vktrgcoW5eCh0qCfTkFnbTFLrhvXrI
   --- rFMmV2H+FgP27oaLC6SHQOLy5d5DPGSp2pktFo/AOh8
   U�`OZ�rGЕ~N}Ͷ
   MbE/2m��`aQfl&$QCx
   n:T?#�k!_�ΉIa�Y|�}j[頙߄)JJ{څ1y}cܪB���7�
   ```

   A public key file:

   ```identity.pub
   age1c3jdepjm05aey2dq9dgkfn4utj9a776zwqzqcar3879smuh04ysqttvmyd
   ```

   A secret key file:

   ```identity.key
   AGE-SECRET-KEY-1N84CR29PJTUQA22ALHP4YDL5ZFMXPW5GVETVY3UK58ZD6NPNPDLS4MCZFS
   ```
* `-K <KEY>` — The key to use for encrypting/decrypting as a string.

   This does not support the age identity format, only single keys.

   When encrypting and provided with a secret key, the corresponding public key
   will be derived first; there is no way to encrypt with a secret key such that
   a file is decodable with the public key.

   There is no support for password-protected secret keys.

   ## Examples

   With a public key:

   ```console
   --key age1c3jdepjm05aey2dq9dgkfn4utj9a776zwqzqcar3879smuh04ysqttvmyd
   ```

   With a secret key:

   ```console
   --key AGE-SECRET-KEY-1N84CR29PJTUQA22ALHP4YDL5ZFMXPW5GVETVY3UK58ZD6NPNPDLS4MCZFS
   ```
* `-P`, `--passphrase-path <PASSPHRASE_PATH>` — Path to a file containing a passphrase.

   The contents of the file will be trimmed of whitespace.
* `--insecure-passphrase <INSECURE_PASSPHRASE>` — A passphrase as a string.

   This is extremely insecure, only use when there is no other option. When on an interactive terminal, make sure to wipe this command line from your history, or better yet not record it in the first place (in Bash you often can do that by prepending a space to your command).



## `algae-cli keygen`

Generate an identity (key pair) to encrypt and decrypt files

This creates a passphrase-protected identity file which contains both public
and secret keys:

```identity.txt
# created: 2024-12-20T05:36:10.267871872+00:00
# public key: age1c3jdepjm05aey2dq9dgkfn4utj9a776zwqzqcar3879smuh04ysqttvmyd
AGE-SECRET-KEY-1N84CR29PJTUQA22ALHP4YDL5ZFMXPW5GVETVY3UK58ZD6NPNPDLS4MCZFS
```

As well as a plaintext public key file which contains just the public key:

```identity.pub
age1c3jdepjm05aey2dq9dgkfn4utj9a776zwqzqcar3879smuh04ysqttvmyd
```

The public key is also printed to stdout.

By default this command prompts for a passphrase. This can be disabled with
`--plaintext`; the default path `identity.txt` instead of `identity.txt.age`
is used if `--output` isn't given, and the contents will be in plain text
(in the format shown above).

On encrypting machines (e.g. servers uploading backups), you should always
prefer to store _just_ the public key, and only upload and use the
passphrase-protected identity file as necessary, deleting it afterwards.

Identity files (both plaintext and passphrase-protected) generated by this
command are compatible with the `age` CLI tool. Note that the reverse might
not be true (there are age-generated identities that this tool cannot handle).

**Usage:** `algae-cli keygen [OPTIONS]`

###### **Options:**

* `-o`, `--output <OUTPUT>` — Path to write the identity file to.

   Defaults to identity.txt.age, and to identity.txt if --plaintext is given.
* `--public <PUBLIC_PATH>` — Path to write the public key file to.

   Set to a single hyphen (`-`) to disable writing this file; the public key will be printed to stdout in any case.

  Default value: `identity.pub`
* `--plaintext` — INSECURE: write a plaintext identity
* `-R`, `--random-passphrase` — Generate a random passphrase.

   Instead of entering a passphrase yourself, this will generate one with random words (from the Minilock wordlist) and print it out for you.
* `-P`, `--passphrase-path <PASSPHRASE_PATH>` — Path to a file containing a passphrase.

   The contents of the file will be trimmed of whitespace.
* `--insecure-passphrase <INSECURE_PASSPHRASE>` — A passphrase as a string.

   This is extremely insecure, only use when there is no other option. When on an interactive terminal, make sure to wipe this command line from your history, or better yet not record it in the first place (in Bash you often can do that by prepending a space to your command).



## `algae-cli protect`

Encrypt a file using a passphrase.

Whenever possible, prefer to use `encrypt` and `decrypt` with identity files (public key cryptography).

This utility may also be used to convert a plaintext identity file into a passphrase-protected one.

**Usage:** `algae-cli protect [OPTIONS] <INPUT>`

###### **Arguments:**

* `<INPUT>` — File to be encrypted

###### **Options:**

* `-o`, `--output <OUTPUT>` — Path or filename to write the encrypted file to.

   By default this is the input file, with `.age` appended.
* `--rm` — Delete input file after encrypting
* `-P`, `--passphrase-path <PASSPHRASE_PATH>` — Path to a file containing a passphrase.

   The contents of the file will be trimmed of whitespace.
* `--insecure-passphrase <INSECURE_PASSPHRASE>` — A passphrase as a string.

   This is extremely insecure, only use when there is no other option. When on an interactive terminal, make sure to wipe this command line from your history, or better yet not record it in the first place (in Bash you often can do that by prepending a space to your command).



## `algae-cli reveal`

Decrypt a file using a passphrase.

Whenever possible, prefer to use `encrypt` and `decrypt` with identity files (public key cryptography).

This utility may also be used to convert a passphrase-protected identity file into a plaintext one.

**Usage:** `algae-cli reveal [OPTIONS] <INPUT>`

###### **Arguments:**

* `<INPUT>` — File to be decrypted

###### **Options:**

* `-o`, `--output <OUTPUT>` — Path or filename to write the decrypted file to.

   If the input file has a `.age` extension, this can be automatically derived (by removing the `.age`). Otherwise, this option is required.
* `-P`, `--passphrase-path <PASSPHRASE_PATH>` — Path to a file containing a passphrase.

   The contents of the file will be trimmed of whitespace.
* `--insecure-passphrase <INSECURE_PASSPHRASE>` — A passphrase as a string.

   This is extremely insecure, only use when there is no other option. When on an interactive terminal, make sure to wipe this command line from your history, or better yet not record it in the first place (in Bash you often can do that by prepending a space to your command).



<hr/>

<small><i>
    This document was generated automatically by
    <a href="https://crates.io/crates/clap-markdown"><code>clap-markdown</code></a>.
</i></small>

