# Command-Line Help for `bestool`

This document contains the help content for the `bestool` command-line program.

**Command Overview:**

* [`bestool`↴](#bestool)
* [`bestool audit-psql`↴](#bestool-audit-psql)
* [`bestool caddy`↴](#bestool-caddy)
* [`bestool caddy configure-tamanu`↴](#bestool-caddy-configure-tamanu)
* [`bestool caddy download`↴](#bestool-caddy-download)
* [`bestool completions`↴](#bestool-completions)
* [`bestool crypto`↴](#bestool-crypto)
* [`bestool crypto decrypt`↴](#bestool-crypto-decrypt)
* [`bestool crypto encrypt`↴](#bestool-crypto-encrypt)
* [`bestool crypto hash`↴](#bestool-crypto-hash)
* [`bestool crypto keygen`↴](#bestool-crypto-keygen)
* [`bestool crypto protect`↴](#bestool-crypto-protect)
* [`bestool crypto reveal`↴](#bestool-crypto-reveal)
* [`bestool file`↴](#bestool-file)
* [`bestool file join`↴](#bestool-file-join)
* [`bestool file split`↴](#bestool-file-split)
* [`bestool iti`↴](#bestool-iti)
* [`bestool iti battery`↴](#bestool-iti-battery)
* [`bestool iti improv-wifi`↴](#bestool-iti-improv-wifi)
* [`bestool iti lcd`↴](#bestool-iti-lcd)
* [`bestool iti lcd serve`↴](#bestool-iti-lcd-serve)
* [`bestool iti lcd send`↴](#bestool-iti-lcd-send)
* [`bestool iti lcd clear`↴](#bestool-iti-lcd-clear)
* [`bestool iti lcd on`↴](#bestool-iti-lcd-on)
* [`bestool iti lcd off`↴](#bestool-iti-lcd-off)
* [`bestool iti sparks`↴](#bestool-iti-sparks)
* [`bestool iti temperature`↴](#bestool-iti-temperature)
* [`bestool rdp`↴](#bestool-rdp)
* [`bestool rdp monitor`↴](#bestool-rdp-monitor)
* [`bestool rdp service`↴](#bestool-rdp-service)
* [`bestool rdp service install`↴](#bestool-rdp-service-install)
* [`bestool rdp service uninstall`↴](#bestool-rdp-service-uninstall)
* [`bestool rdp service start`↴](#bestool-rdp-service-start)
* [`bestool rdp service stop`↴](#bestool-rdp-service-stop)
* [`bestool rdp service status`↴](#bestool-rdp-service-status)
* [`bestool self-update`↴](#bestool-self-update)
* [`bestool ssh`↴](#bestool-ssh)
* [`bestool ssh add-key`↴](#bestool-ssh-add-key)
* [`bestool tamanu`↴](#bestool-tamanu)
* [`bestool tamanu alerts`↴](#bestool-tamanu-alerts)
* [`bestool tamanu alertd`↴](#bestool-tamanu-alertd)
* [`bestool tamanu alertd run`↴](#bestool-tamanu-alertd-run)
* [`bestool tamanu alertd status`↴](#bestool-tamanu-alertd-status)
* [`bestool tamanu alertd reload`↴](#bestool-tamanu-alertd-reload)
* [`bestool tamanu alertd loaded-alerts`↴](#bestool-tamanu-alertd-loaded-alerts)
* [`bestool tamanu alertd pause-alert`↴](#bestool-tamanu-alertd-pause-alert)
* [`bestool tamanu alertd validate`↴](#bestool-tamanu-alertd-validate)
* [`bestool tamanu artifacts`↴](#bestool-tamanu-artifacts)
* [`bestool tamanu backup`↴](#bestool-tamanu-backup)
* [`bestool tamanu backup-configs`↴](#bestool-tamanu-backup-configs)
* [`bestool tamanu config`↴](#bestool-tamanu-config)
* [`bestool tamanu db-url`↴](#bestool-tamanu-db-url)
* [`bestool tamanu doctor`↴](#bestool-tamanu-doctor)
* [`bestool tamanu download`↴](#bestool-tamanu-download)
* [`bestool tamanu find`↴](#bestool-tamanu-find)
* [`bestool tamanu greenmask-config`↴](#bestool-tamanu-greenmask-config)
* [`bestool tamanu logs`↴](#bestool-tamanu-logs)
* [`bestool tamanu meta-ticket`↴](#bestool-tamanu-meta-ticket)
* [`bestool tamanu psql`↴](#bestool-tamanu-psql)
* [`bestool tamanu restart`↴](#bestool-tamanu-restart)
* [`bestool tamanu start`↴](#bestool-tamanu-start)
* [`bestool tamanu status`↴](#bestool-tamanu-status)
* [`bestool tamanu stop`↴](#bestool-tamanu-stop)

## `bestool`

BES Tooling

**Usage:** `bestool [OPTIONS] <COMMAND>`

Didn't expect this much output? Use the short '-h' flag to get short help.

###### **Subcommands:**

* `audit-psql` — Export audit database entries as JSON
* `caddy` — Manage Caddy
* `completions` — Generate a shell completions script
* `crypto` — Cryptographic operations
* `file` — File utilities
* `iti` — Tamanu Iti subcommands
* `rdp` — Windows RDP session tooling
* `self-update` — Update this bestool
* `ssh` — SSH helpers
* `tamanu` — Interact with Tamanu

###### **Options:**

* `--color <MODE>` — When to use terminal colours.

   You can also set the `NO_COLOR` environment variable to disable colours, or the `CLICOLOR_FORCE` environment variable to force colours. Defaults to `auto`, which checks whether the output is a terminal to decide.

  Default value: `auto`

  Possible values:
  - `auto`:
    Automatically detect whether to use colours
  - `always`:
    Always use colours, even if the terminal does not support them
  - `never`:
    Never use colours

* `-v`, `--verbose` — Set diagnostic log level.

   This enables diagnostic logging, which is useful for investigating bugs. Use multiple times to increase verbosity.

   You may want to use with `--log-file` to avoid polluting your terminal.

  Default value: `0`
* `--log-file <PATH>` — Write diagnostic logs to a file.

   This writes diagnostic logs to a file, instead of the terminal, in JSON format.

   If the path provided is a directory, a file will be created in that directory with daily rotation. The initial file name will be in the format `programname.YYYY-MM-DDTHH-MM-SSZ.log`, and a new file will be created each day at midnight UTC.

   If the path is a file, logs will be written to that specific file without rotation.
* `--log-file-keep <COUNT>` — Limit the number of log files to keep.

   When used with a directory in `--log-file`, this controls how many rotated log files are kept. Older files are automatically deleted when this limit is reached. Defaults to 32 days of logs. Pass 0 to disable rotation and keep all files.

  Default value: `32`
* `--log-timeless` — Omit timestamps in logs.

   This can be useful when running under service managers that capture logs, to avoid having two timestamps. When run under systemd, this is automatically enabled.

   This option is ignored if the log file is set, or when using `RUST_LOG` or equivalent (as logging is initialized before arguments are parsed in that case); you may want to use `LOG_TIMELESS` instead in the latter case.



## `bestool audit-psql`

Export audit database entries as JSON

**Usage:** `bestool audit-psql [OPTIONS]`

###### **Options:**

* `--audit-path <PATH>` — Path to audit database directory (default: ~/.local/state/bestool-psql)
* `-n`, `--limit <LIMIT>` — Number of entries to return (0 = unlimited)

  Default value: `100`
* `--first` — Read from oldest entries instead of newest
* `--since <SINCE>` — Filter entries after this date
* `--until <UNTIL>` — Filter entries before this date
* `--orphans` — Discover and read orphan databases instead of main database



## `bestool caddy`

Manage Caddy

**Usage:** `bestool caddy <COMMAND>`

###### **Subcommands:**

* `configure-tamanu` — Configure Caddy for a Tamanu installation
* `download` — Download caddy



## `bestool caddy configure-tamanu`

Configure Caddy for a Tamanu installation

**Usage:** `bestool caddy configure-tamanu [OPTIONS] --domain <DOMAIN> --api-port <PORT> --api-version <VERSION> --web-version <VERSION>`

###### **Options:**

* `--path <PATH>` — Path to the Caddyfile

  Default value: `/etc/caddy/Caddyfile`
* `--print` — Print the Caddyfile, don't write it to disk
* `--domain <DOMAIN>` — Tamanu domain name
* `--api-port <PORT>` — Tamanu API server port
* `--api-version <VERSION>` — Tamanu server version to configure
* `--web-version <VERSION>` — Tamanu frontend version to configure
* `--email <EMAIL>` — Email for TLS issuance
* `--zerossl-api-key <ZEROSSL_API_KEY>` — ZeroSSL API Key.

   If not provided, ZeroSSL will still be used as per default Caddy config, but rate limited.



## `bestool caddy download`

Download caddy

**Usage:** `bestool caddy download [OPTIONS] --path <PATH> [VERSION]`

###### **Arguments:**

* `<VERSION>` — Version to download

  Default value: `latest`

###### **Options:**

* `--path <PATH>` — Where to download to
* `--url-only` — Print the URL, don't download.

   Useful if you want to download it on a different machine, or with a different tool.
* `--target <TARGET>` — Target to download.

   Usually the auto-detected default is fine, in rare cases you may need to override it.



## `bestool completions`

Generate a shell completions script.

Provides a completions script or configuration for the given shell.

**Usage:** `bestool completions --shell <SHELL>`

###### **Options:**

* `--shell <SHELL>` — Shell to generate a completions script for

  Possible values: `bash`, `elvish`, `fish`, `nu`, `powershell`, `zsh`




## `bestool crypto`

Cryptographic operations

**Usage:** `bestool crypto <COMMAND>`

###### **Subcommands:**

* `decrypt` — Decrypt a file using a secret key or an identity
* `encrypt` — Encrypt a file using a public key or an identity
* `hash` — Checksum files and folders
* `keygen` — Generate an identity (key pair) to encrypt and decrypt files
* `protect` — Encrypt a file using a passphrase
* `reveal` — Decrypt a file using a passphrase



## `bestool crypto decrypt`

Decrypt a file using a secret key or an identity.

Either of `--key-path` or `--key` must be provided.

For symmetric cryptography (using a passphrase), see `protect`/`reveal`.

**Usage:** `bestool crypto decrypt [OPTIONS] <INPUT>`

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



## `bestool crypto encrypt`

Encrypt a file using a public key or an identity.

Either of `--key-path` or `--key` must be provided.

For symmetric cryptography (using a passphrase), see `protect`/`reveal`.

**Usage:** `bestool crypto encrypt [OPTIONS] <INPUT>`

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



## `bestool crypto hash`

Checksum files and folders.

This uses the BLAKE3 algorithm and expects digests to be prefixed by `b3:` to be future-proof.

**Usage:** `bestool crypto hash [OPTIONS] <PATHS>...`

###### **Arguments:**

* `<PATHS>` — Paths to files and/or folders to compute a checksum for.

   One path will generate one checksum.

###### **Options:**

* `--check <CHECKS>` — Digests to check the generated ones against.

   Must be provided in the same order as the inputs.
* `-n`, `--no-filenames` — Print just the hashes, not the filenames



## `bestool crypto keygen`

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

**Usage:** `bestool crypto keygen [OPTIONS]`

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



## `bestool crypto protect`

Encrypt a file using a passphrase.

Whenever possible, prefer to use `encrypt` and `decrypt` with identity files (public key cryptography).

This utility may also be used to convert a plaintext identity file into a passphrase-protected one.

**Usage:** `bestool crypto protect [OPTIONS] <INPUT>`

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



## `bestool crypto reveal`

Decrypt a file using a passphrase.

Whenever possible, prefer to use `encrypt` and `decrypt` with identity files (public key cryptography).

This utility may also be used to convert a passphrase-protected identity file into a plaintext one.

**Usage:** `bestool crypto reveal [OPTIONS] <INPUT>`

###### **Arguments:**

* `<INPUT>` — File to be decrypted

###### **Options:**

* `-o`, `--output <OUTPUT>` — Path or filename to write the decrypted file to.

   If the input file has a `.age` extension, this can be automatically derived (by removing the `.age`). Otherwise, this option is required.
* `-P`, `--passphrase-path <PASSPHRASE_PATH>` — Path to a file containing a passphrase.

   The contents of the file will be trimmed of whitespace.
* `--insecure-passphrase <INSECURE_PASSPHRASE>` — A passphrase as a string.

   This is extremely insecure, only use when there is no other option. When on an interactive terminal, make sure to wipe this command line from your history, or better yet not record it in the first place (in Bash you often can do that by prepending a space to your command).



## `bestool file`

File utilities

**Usage:** `bestool file <COMMAND>`

###### **Subcommands:**

* `join` — Join a split file
* `split` — Split a file into fixed-size chunks



## `bestool file join`

Join a split file.

This is the counter to `bestool file split`.

Chunked files can be joined very simply using `cat`. However, this will not verify integrity. This subcommand checks that all chunks are present, that each chunk matches its checksum, and that the whole file matches that checksum as well, while writing the joined file.

As a result, it is also quite a bit slower than `cat`; if you trust the input, you may want to use that instead for performance.

**Usage:** `bestool file join <INPUT> [OUTPUT]`

###### **Arguments:**

* `<INPUT>` — Path to the directory of chunks to be joined
* `<OUTPUT>` — Path to the output directory or file.

   If a directory is given, this cannot be the same directory as contains the input chunked directory; the name of the directory will be used as the output filename.

   If not provided, and stdout is NOT a terminal, the output will be streamed there. Note that in that case, you should pay attention to the exit code: if it is not success, integrity checks may have failed and you should discard the obtained output.



## `bestool file split`

Split a file into fixed-size chunks.

We sometimes deal with very large files. Uploading them in one go over an unreliable connection can be a painful experience, and in some cases not succeed. This option provides a lo-fi solution to the problem, by splitting a file into smaller chunks. It is then a lot easier to upload the chunks and retry on error or after network failures by re-uploading chunks missing on the remote; `rclone sync` can do this for example.

The file chunks are written into a directory named after the original file, including the extension. This makes the remote's job simpler: take all the chunks and re-assemble into one file, naming it the same as the containing directory.

A metadata file is also written. This is a JSON file which contains the number of chunks created, a checksum over the whole file, and a checksum for each chunk. This can be used by the re-assembler to check whether all chunks are available, and verify integrity. The `join` sibling subcommand provides such a re-assembler, or you can simply use `cat` (without integrity checks).

The checksums are compatible with the ones written and verified by the `crypto hash` subcommand.

**Usage:** `bestool file split [OPTIONS] <INPUT> <OUTPUT>`

###### **Arguments:**

* `<INPUT>` — Path to the file to be split
* `<OUTPUT>` — Path to the output directory.

   Cannot be the same directory as contains the input file.

###### **Options:**

* `-s`, `--size <SIZE>` — The chunk size in mibibytes.

   Takes a non-zero integer size in mibibytes.

   If not present, the default is to pick a chunk size between 8 MiB and 64 MiB inclusive, such that the input file is divided in 1000 chunks. The resulting size is rounded to the nearest 8 KiB, to make copying and disk usage more efficient.



## `bestool iti`

Tamanu Iti subcommands

**Usage:** `bestool iti <COMMAND>`

###### **Subcommands:**

* `battery` — Get battery information from the X1201 board
* `improv-wifi` — Run the Improv-Wi-Fi BLE peripheral so a phone or browser can provision the device's Wi-Fi
* `lcd` — Control an LCD screen
* `sparks` — Display CPU and memory usage as spark lines on the LCD
* `temperature` — Get core temperature from the Raspberry Pi



## `bestool iti battery`

Get battery information from the X1201 board

**Usage:** `bestool iti battery [OPTIONS] [ZMQ_SOCKET]`

###### **Arguments:**

* `<ZMQ_SOCKET>` — ZMQ socket to use for screen updates

  Default value: `tcp://[::1]:2009`

###### **Options:**

* `--json` — Output in JSON format
* `--update-screen <UPDATE_SCREEN>` — Update screen with battery status.

   Argument is the Y position of the battery status. The X position is always 240 (right edge).

   With --estimate, this will also print the time remaining on the left edge (X=20).
* `--watch <WATCH>` — Keep updating at an interval.

   Syntax is a number followed by a unit, such as "5s" or "1m".
* `--estimate` — With --watch, also estimate charging rate and time remaining.

   The first round will be estimate-less, as it is used to gather data. After that, the rate and time remaining (in seconds in the JSON output) are calculated on a rolling basis.



## `bestool iti improv-wifi`

Run the Improv-Wi-Fi BLE peripheral so a phone or browser can provision the device's Wi-Fi.

Uses BlueZ for BLE and NetworkManager for Wi-Fi configuration.

Default mode is a long-running daemon that advertises only on demand (a fresh device with no Wi-Fi config advertises immediately for first-boot provisioning; once provisioned, the device stays idle until a long-press on `--auth-gpio` re-enters provisioning mode). Use `--one-shot` for the legacy single-provisioning behaviour.

**Usage:** `bestool iti improv-wifi [OPTIONS]`

###### **Options:**

* `--adapter <ADAPTER>` — Bluetooth adapter to use (e.g. `hci0`). Defaults to the system's first powered adapter
* `--local-name <LOCAL_NAME>` — Local name advertised over BLE. Defaults to the system hostname
* `--device-name <DEVICE_NAME>` — Device name reported in Device Info / Device Name commands. Defaults to the system hostname
* `--auth-stdin` — Authorise when a line is received on stdin (the line content is ignored).

   When set, the device starts in `AuthorizationRequired` and only accepts credentials after the first line on stdin. Only valid with `--one-shot`.
* `--auth-gpio <AUTH_GPIO>` — Authorise on a button press on this BCM GPIO pin.

   The pin is configured as input with the internal pull-up resistor; wire a momentary switch from the pin to GND.

   In default (daemon) mode this is the long-press trigger to enter provisioning mode and the short-press trigger to authorise an in-progress session. In `--one-shot` mode any press authorises the single session.
* `--auth-gpio-debounce <AUTH_GPIO_DEBOUNCE>` — Debounce window for `--auth-gpio`

  Default value: `50ms`
* `--auth-gpio-long-press <AUTH_GPIO_LONG_PRESS>` — Hold time on `--auth-gpio` that counts as a long press (daemon mode only)

  Default value: `3s`
* `--auth-timeout <AUTH_TIMEOUT>` — How long an authorisation stays valid before the device reverts to `AuthorizationRequired`. If unset, the device stays authorised until provisioned or shut down
* `--no-auth` — Skip authorisation gating: start the advertising session in `Authorized` and accept credentials from any device in BLE range without requiring a button press or stdin input.

   SECURITY WARNING: this removes the physical-presence guarantee. Any device in BLE range during an advertising session can overwrite the device's Wi-Fi configuration. Requires `--one-shot`.
* `--always` — Run even if Wi-Fi is already connected.

   In `--one-shot` mode, the command exits cleanly when NetworkManager reports the Wi-Fi device is in the `Activated` state. Pass this flag to override that check.
* `--one-shot` — Run a single provisioning session and exit, instead of staying alive as a daemon.

   SECURITY WARNING: while running, the device advertises over BLE until it is provisioned, expanding the BLE attack surface. The default daemon mode is invisible after provisioning and only re-enters advertising on a long-press of `--auth-gpio`.



## `bestool iti lcd`

Control an LCD screen.

This is made for Waveshare's 1.69 inch LCD display, connected over SPI to a Raspberry Pi.

See more info about it here: https://www.waveshare.com/wiki/1.69inch_LCD_Module

You'll want to set up SPI's buffer size by adding `spidev.bufsiz=131072` to `/boot/firmware/cmdline.txt`, otherwise you'll get "Message too long" errors.

**Usage:** `bestool iti lcd [OPTIONS] [ZMQ_SOCKET] <COMMAND>`

###### **Subcommands:**

* `serve` — Start the LCD display server
* `send` — Send an arbitrary JSON message to the display server
* `clear` — Set all pixels to a single color
* `on` — Turn the display on
* `off` — Turn the display off

###### **Arguments:**

* `<ZMQ_SOCKET>` — ZMQ socket to use for JSON screen updates

  Default value: `tcp://[::1]:2009`

###### **Options:**

* `--spi <SPI>` — SPI port to use

  Default value: `0`
* `--backlight <BACKLIGHT>` — GPIO pin number for the display's backlight control pin

  Default value: `18`
* `--reset <RESET>` — GPIO pin number for the display's reset pin

  Default value: `27`
* `--dc <DC>` — GPIO pin number for the display's data/command pin

  Default value: `25`
* `--ce <CE>` — SPI CE number for the display's chip select pin

  Default value: `0`
* `--frequency <FREQUENCY>` — SPI frequency in Hz

  Default value: `20000000`



## `bestool iti lcd serve`

Start the LCD display server.

This will initiatialize the LCD display, listen for JSON messages on a ZMQ REP socket, and update the display based on the contents of the messages.

Note that enabling trace-level (`-vvv`) logging will considerably slow down screen updates, as it will log every command sent to the screen, which can be considerable for complex layouts and text.

**Usage:** `bestool iti lcd serve`



## `bestool iti lcd send`

Send an arbitrary JSON message to the display server.

This is useful for debugging or testing the display server, or for interacting with the screen without a ZMQ client.

The message can be provided either as the first argument, or over stdin.

The message will be validated by the client to avoid sending malformed messages to the server. The command will block until the message can be sent to the display server, then wait for a reply and print it if non-empty.

**Usage:** `bestool iti lcd send [MESSAGE]`

###### **Arguments:**

* `<MESSAGE>` — JSON message to send



## `bestool iti lcd clear`

Set all pixels to a single color.

The command will block until the message can be sent to the display server, then wait for a reply and print it if non-empty.

**Usage:** `bestool iti lcd clear [RED] [GREEN] [BLUE]`

###### **Arguments:**

* `<RED>` — Red value for the background color

  Default value: `0`
* `<GREEN>` — Green value for the background color

  Default value: `0`
* `<BLUE>` — Blue value for the background color

  Default value: `0`



## `bestool iti lcd on`

Turn the display on.

This wakes the display, turns on the backlight, and shows the current screen contents.

The LCD must then rest for 120ms before any further commands can be sent.

The command will block until the message can be sent to the display server, then wait for a reply and print it if non-empty.

**Usage:** `bestool iti lcd on`



## `bestool iti lcd off`

Turn the display off.

This turns off the backlight and puts the display to sleep, which uses less power.

The LCD must then rest for 5ms before any further commands can be sent.

The command will block until the message can be sent to the display server, then wait for a reply and print it if non-empty.

**Usage:** `bestool iti lcd off`



## `bestool iti sparks`

Display CPU and memory usage as spark lines on the LCD

**Usage:** `bestool iti sparks [OPTIONS] [ZMQ_SOCKET]`

###### **Arguments:**

* `<ZMQ_SOCKET>` — ZMQ socket to use for screen updates

  Default value: `tcp://[::1]:2009`

###### **Options:**

* `--y <Y>` — Y position of the gauges

  Default value: `30`
* `--h <H>` — Height of the gauges

  Default value: `27`
* `--interval <INTERVAL>` — Refresh interval

  Default value: `10s`



## `bestool iti temperature`

Get core temperature from the Raspberry Pi

**Usage:** `bestool iti temperature [OPTIONS] [ZMQ_SOCKET]`

###### **Arguments:**

* `<ZMQ_SOCKET>` — ZMQ socket to use for screen updates

  Default value: `tcp://[::1]:2009`

###### **Options:**

* `--json` — Output in JSON format
* `--update-screen <UPDATE_SCREEN>` — Update screen with temperature.

   Argument is the Y position of the temperature display. The X position is always 240 (right edge).
* `--watch <WATCH>` — Keep updating at an interval.

   Syntax is a number followed by a unit, such as "5s" or "1m".



## `bestool rdp`

Windows RDP session tooling

**Usage:** `bestool rdp <COMMAND>`

###### **Subcommands:**

* `monitor` — Watch RDP sessions and notify on fast user-switch ("kick")
* `service` — Install, remove, start, stop, or query the `bestool-rdp-monitor` Windows Service. All subcommands except `status` require Administrator rights



## `bestool rdp monitor`

Watch RDP sessions and notify on fast user-switch ("kick").

Runs a long-lived loop that polls the TerminalServices event log for session connect/disconnect events, cross-references source IPs with Tailscale to identify users, and raises a Windows toast on the incoming session when a different user was connected moments before.

Intended to run as a Windows service or startup task with sufficient privilege to read the TerminalServices-LocalSessionManager log (typically LocalSystem or Administrators).

**Usage:** `bestool rdp monitor [OPTIONS]`

###### **Options:**

* `--audit-log <AUDIT_LOG>` — Path to append-only JSONL audit log of every RDP session event

  Default value: `C:\ProgramData\bestool\rdp-audit.jsonl`
* `--poll-interval <POLL_INTERVAL>` — Seconds between event log polls

  Default value: `3`
* `--kick-window <KICK_WINDOW>` — Max seconds between a disconnect and a new logon to count as a "kick" and raise a toast

  Default value: `60`



## `bestool rdp service`

Install, remove, start, stop, or query the `bestool-rdp-monitor` Windows Service. All subcommands except `status` require Administrator rights

**Usage:** `bestool rdp service <COMMAND>`

###### **Subcommands:**

* `install` — Register the service with the Service Control Manager (auto-start)
* `uninstall` — Remove the service from the Service Control Manager
* `start` — Start the installed service
* `stop` — Stop the running service
* `status` — Print the current service state



## `bestool rdp service install`

Register the service with the Service Control Manager (auto-start)

**Usage:** `bestool rdp service install [OPTIONS]`

###### **Options:**

* `--audit-log <AUDIT_LOG>` — Path to append-only JSONL audit log of every RDP session event
* `--poll-interval <POLL_INTERVAL>` — Seconds between event log polls
* `--kick-window <KICK_WINDOW>` — Max seconds between a disconnect and a new logon to count as a "kick"



## `bestool rdp service uninstall`

Remove the service from the Service Control Manager

**Usage:** `bestool rdp service uninstall`



## `bestool rdp service start`

Start the installed service

**Usage:** `bestool rdp service start`



## `bestool rdp service stop`

Stop the running service

**Usage:** `bestool rdp service stop`



## `bestool rdp service status`

Print the current service state

**Usage:** `bestool rdp service status`



## `bestool self-update`

Update this bestool.

Alias: self

**Usage:** `bestool self-update [OPTIONS]`

###### **Options:**

* `--version <VERSION>` — Version to update to

  Default value: `latest`
* `--target <TARGET>` — Target to download.

   Usually the auto-detected default is fine, in rare cases you may need to override it.
* `--temp-dir <TEMP_DIR>` — Temporary directory to download to.

   Defaults to the system temp directory.
* `--force` — Force reinstall, even if already on the latest version or installed via package manager



## `bestool ssh`

SSH helpers

**Usage:** `bestool ssh <COMMAND>`

###### **Subcommands:**

* `add-key` — Add a public key to the current user's authorized_keys file



## `bestool ssh add-key`

Add a public key to the current user's authorized_keys file.

On Unix, this is equivalent to `echo 'public key' >> ~/.ssh/authorized_keys`, except that this command will check public keys are well-formed and will never accidentally overwrite the file.

On Windows, this behaves differently whether the current user is a regular user or an administrator, as the file that needs to be written is different. Additionally, it will ensure that file ACLs are correct when used for administrators.

This tool will obtain an exclusive lock on the file to prevent concurrent modification, which could result in a loss of data. It will also check the validity of the file before writing it.

**Usage:** `bestool ssh add-key <KEYS>...`

###### **Arguments:**

* `<KEYS>` — SSH public key to add.

   Multiple keys may be provided, which will behave the same as calling this command multiple times with each different key.

   Keys that already exist are automatically excluded so they're not written twice.



## `bestool tamanu`

Interact with Tamanu.

Alias: t

**Usage:** `bestool tamanu [OPTIONS] <COMMAND>`

###### **Subcommands:**

* `alerts` — Execute alert definitions against Tamanu
* `alertd` — Run the alert daemon
* `artifacts` — List available artifacts for a Tamanu version
* `backup` — Backup a local Tamanu database to a single file
* `backup-configs` — Backup local Tamanu-related config files to a zip archive
* `config` — Find and print the current Tamanu config
* `db-url` — Generate a DATABASE_URL connection string
* `doctor` — Gather server info + healthchecks for a Tamanu install
* `download` — Download Tamanu artifacts
* `find` — Find Tamanu installations
* `greenmask-config` — Generate a Greenmask config file
* `logs` — Tail logs for tamanu services and (optionally) caddy.
* `meta-ticket` — Generate a meta-ticket for this Tamanu server
* `psql` — Connect to Tamanu's database
* `restart` — Rolling-restart all running tamanu services.
* `start` — Bring up any expected tamanu services that aren't running.
* `status` — Report on tamanu services: what's expected vs what's actually running.
* `stop` — Stop running tamanu services.

###### **Options:**

* `--root <ROOT>` — Tamanu root to operate in



## `bestool tamanu alerts`

Execute alert definitions against Tamanu

DEPRECATED. Use `bestool tamanu alertd` for all new deployments.

The alert and target definitions are documented online at:
<https://github.com/beyondessential/bestool/blob/main/crates/alertd/ALERTS.md>
and <https://github.com/beyondessential/bestool/blob/main/crates/alertd/TARGETS.md>.

**Usage:** `bestool tamanu alerts [OPTIONS]`

###### **Options:**

* `--dir <DIR>` — Folder containing alert definitions.

   This folder will be read recursively for files with the `.yaml` or `.yml` extension.

   Files that don't match the expected format will be skipped, as will files with `enabled: false` at the top level. Syntax errors will be reported for YAML files.

   It's entirely valid to provide a folder that only contains a `_targets.yml` file.

   Can be provided multiple times. Defaults to (depending on platform): `C:\Tamanu\alerts`, `C:\Tamanu\{current-version}\alerts`, `/opt/tamanu-toolbox/alerts`, `/etc/tamanu/alerts`, `/alerts`, and `./alerts`.
* `--interval <INTERVAL>` — How far back to look for alerts.

   This is a duration string, e.g. `1d` for one day, `1h` for one hour, etc. It should match the task scheduling / cron interval for this command.

  Default value: `15m`
* `--timeout <TIMEOUT>` — Timeout for each alert.

   If an alert takes longer than this to query the database or run the shell script, it will be skipped. Defaults to 30 seconds.

   This is a duration string, e.g. `1d` for one day, `1h` for one hour, etc.

  Default value: `30s`
* `--dry-run` — Don't actually send alerts, just print them to stdout



## `bestool tamanu alertd`

Run the alert daemon

The alert and target definitions are documented online at:
<https://github.com/beyondessential/bestool/blob/main/crates/alertd/ALERTS.md>
and <https://github.com/beyondessential/bestool/blob/main/crates/alertd/TARGETS.md>.

Configuration for database and email is read from Tamanu's config files.

**Usage:** `bestool tamanu alertd <COMMAND>`

###### **Subcommands:**

* `run` — Run the alert daemon
* `status` — Show status and health of a running daemon
* `reload` — Send reload signal to running daemon
* `loaded-alerts` — List currently loaded alert files
* `pause-alert` — Temporarily pause an alert
* `validate` — Validate an alert definition file



## `bestool tamanu alertd run`

Run the alert daemon

Starts the daemon which monitors alert definition files and executes alerts based on their configured schedules. The daemon will watch for file changes and automatically reload when definitions are modified.

**Usage:** `bestool tamanu alertd run [OPTIONS]`

###### **Options:**

* `--glob <GLOB>` — Glob patterns for alert definitions

   Patterns can match directories (which will be read recursively) or individual files. Can be provided multiple times. Examples: /etc/tamanu/alerts, /opt/*/alerts, /etc/tamanu/alerts/**/*.yml
* `--dry-run` — Execute all alerts once and quit (ignoring intervals)
* `--no-server` — Disable the HTTP server
* `--server-addr <SERVER_ADDR>` — HTTP server bind address(es)

   Can be provided multiple times. The server will attempt to bind to each address in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271
* `--watchdog-timeout <WATCHDOG_TIMEOUT>` — Watchdog timeout in seconds

   If no alert task reports activity within this many seconds, the daemon will exit so the service manager can restart it. Defaults to 600 (10 minutes).

  Default value: `600`
* `--no-watchdog` — Disable the watchdog

   By default, the daemon will exit if no alert activity is detected within the watchdog timeout. This flag disables that behavior.
* `--no-healthchecks` — Disable the periodic doctor healthcheck sweep

   By default, the daemon runs the full doctor check registry every minute and posts the result to canopy. This flag turns that off.



## `bestool tamanu alertd status`

Show status and health of a running daemon

Connects to the running daemon's HTTP API and displays version, uptime, health, and watchdog information. Exits with code 1 if the daemon is unhealthy.

**Usage:** `bestool tamanu alertd status [OPTIONS]`

###### **Options:**

* `--server-addr <SERVER_ADDR>` — HTTP server address(es) to try

   Can be provided multiple times. Will attempt to connect to each address in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271



## `bestool tamanu alertd reload`

Send reload signal to running daemon

Connects to the running daemon's HTTP API and triggers a reload. This is an alternative to SIGHUP that works on all platforms including Windows.

**Usage:** `bestool tamanu alertd reload [OPTIONS]`

###### **Options:**

* `--server-addr <SERVER_ADDR>` — HTTP server address(es) to try

   Can be provided multiple times. Will attempt to connect to each address in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271



## `bestool tamanu alertd loaded-alerts`

List currently loaded alert files

Connects to the running daemon's HTTP API and retrieves the list of currently loaded alert definition files.

**Usage:** `bestool tamanu alertd loaded-alerts [OPTIONS]`

###### **Options:**

* `--server-addr <SERVER_ADDR>` — HTTP server address(es) to try

   Can be provided multiple times. Will attempt to connect to each address in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271
* `--detail` — Show detailed state information for each alert



## `bestool tamanu alertd pause-alert`

Temporarily pause an alert

Pauses an alert until the specified time. The alert will not execute during this period. The pause is lost when the daemon restarts.

**Usage:** `bestool tamanu alertd pause-alert [OPTIONS] <ALERT>`

###### **Arguments:**

* `<ALERT>` — Alert file path to pause

###### **Options:**

* `--until <UNTIL>` — Time until which to pause the alert (fuzzy time format)

   Examples: "1 hour", "2 days", "next monday", "2024-12-25T10:00:00Z" Defaults to 1 week from now if not specified.
* `--server-addr <SERVER_ADDR>` — HTTP server address(es) to try

   Can be provided multiple times. Will attempt to connect to each address in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271



## `bestool tamanu alertd validate`

Validate an alert definition file

Parses an alert definition file and reports any syntax or validation errors. Uses pretty error reporting to pinpoint the exact location of problems. Requires the daemon to be running.

**Usage:** `bestool tamanu alertd validate [OPTIONS] <FILE>`

###### **Arguments:**

* `<FILE>` — Path to the alert definition file to validate

###### **Options:**

* `--server-addr <SERVER_ADDR>` — HTTP server address(es) to try

   Can be provided multiple times. Will attempt to connect to each address in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271



## `bestool tamanu artifacts`

List available artifacts for a Tamanu version.

Fetches and displays the available artifacts (downloads) for a specific Tamanu version.

Alias: art

**Usage:** `bestool tamanu artifacts [OPTIONS] <VERSION>`

###### **Arguments:**

* `<VERSION>` — Version to list artifacts for

###### **Options:**

* `-p`, `--platform <PLATFORM>` — Platform to list artifacts for.

   Use `host` (default) for the auto-detected current platform, `container` for container artifacts, `os-arch` for specific targets (e.g., `linux-x86_64`), and `all` to list all platforms.

  Default value: `host`



## `bestool tamanu backup`

Backup a local Tamanu database to a single file.

This finds the database from the Tamanu's configuration. The output will be written to a file "{current_datetime}-{host_name}-{database_name}.dump".

By default, this excludes tables "sync_snapshots.*" and "fhir.jobs".

If `--key` or `--key-file` is provided, the backup file will be encrypted. Note that this is done by first writing the plaintext backup file to disk, then encrypting, and finally deleting the original. That effectively requires double the available disk space, and the plaintext file is briefly available on disk. This limitation may be lifted in the future.

Alias: b

**Usage:** `bestool tamanu backup [OPTIONS] [ARGS]...`

###### **Arguments:**

* `<ARGS>` — Additional, arbitrary arguments to pass to "pg_dump"

   If it has dashes (like "--password pass"), you need to prefix this with two dashes:

   ```plain
   bestool tamanu backup -- --password pass
   ```

###### **Options:**

* `--compression-level <COMPRESSION_LEVEL>` — The compression level to use.

   This is simply passed to the "--compress" option of "pg_dump".

  Default value: `3`
* `--write-to <WRITE_TO>` — The destination directory the output will be written to

  Default value: `/opt/tamanu-backup`
* `--then-copy-to <THEN_COPY_TO>` — The file path to copy the written backup.

   The backup will stay as is in "write_to".
* `--then-split <THEN_SPLIT>` — Split the copied file into fixed-sized chunks.

   This is the same as the subcommand `bestool file split`, and the argument is the same as its `--size` option (integer size in mibibytes), except for the special value `0` which behaves as when the upstream subcommand's `--size` option is not provided (size auto-determination).

   Splitting happens after encryption, if enabled.
* `--lean` — Take a lean backup instead.

   The lean backup excludes more tables: "logs.*", "reporting.*" and "public.attachments".

   These thus are not suitable for recovery, but can be used for analysis.

  Default value: `false`
* `--keep-days <KEEP_DAYS>` — Delete backups and copies that are older than N days.

   Only files with the `.dump` or the `.dump.age` extensions are deleted. Subfolders are not recursed into.

   If this option is not provided, a single backup is taken and no deletions are executed.

   Backup deletion always occurs after the backup is taken, so that if the process fails for some reason, existing (presumed valid) backups remain.

   If `--then-copy-to` is provided, also deletes backup files there.
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



## `bestool tamanu backup-configs`

Backup local Tamanu-related config files to a zip archive.

The output will be written to a file "{current_datetime}-{host_name}.config.zip".

If `--key` or `--key-file` is provided, the backup file will be encrypted. Note that this is done by first writing the plaintext backup file to disk, then encrypting, and finally deleting the original. That effectively requires double the available disk space, and the plaintext file is briefly available on disk. This limitation may be lifted in the future.

**Usage:** `bestool tamanu backup-configs [OPTIONS]`

###### **Options:**

* `--write-to <WRITE_TO>` — The destination directory the output will be written to

  Default value: `/opt/tamanu-backup/config`
* `--then-copy-to <THEN_COPY_TO>` — The file path to copy the written backup.

   The backup will stay as is in "write_to".
* `--keep-days <KEEP_DAYS>` — Delete backups and copies that are older than N days.

   Only files with the `.config.zip` or the `.config.zip.age` extensions are deleted. Subfolders are not recursed into.

   If this option is not provided, a single backup is taken and no deletions are executed.

   Backup deletion always occurs after the backup is taken, so that if the process fails for some reason, existing (presumed valid) backups remain.

   If `--then-copy-to` is provided, also deletes backup files there.
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



## `bestool tamanu config`

Find and print the current Tamanu config.

Alias: c

**Usage:** `bestool tamanu config [OPTIONS]`

###### **Options:**

* `-p`, `--package <PACKAGE>` — Package to look at

   If not provided, will look first for central then facility package.
* `-c`, `--compact` — Print compact JSON instead of pretty
* `-n`, `--or-null` — Print null if key not found
* `-k`, `--key <KEY>` — Path to a subkey
* `-r`, `--raw` — If the value is a string, print it directly (without quotes)



## `bestool tamanu db-url`

Generate a DATABASE_URL connection string

This command reads the Tamanu configuration and outputs a PostgreSQL connection string in the standard DATABASE_URL format: `postgresql://user:password@host/database`.

Aliases: db, u, url

**Usage:** `bestool tamanu db-url [OPTIONS]`

###### **Options:**

* `-U`, `--username <USERNAME>` — Database user to use in the connection string.

   If the value matches one of the report schema connection names (e.g., "raw", "reporting"), credentials will be taken from that connection.



## `bestool tamanu doctor`

Gather server info + healthchecks for a Tamanu install

Runs a set of healthchecks against the local Tamanu install and renders a
colour-coded summary. The alertd daemon runs the same checks every minute
and pushes results to Canopy; this command is for interactive operator use.

Exit code 0 on HEALTHY or DEGRADED, 1 on FAILING.

**Usage:** `bestool tamanu doctor [OPTIONS]`

###### **Options:**

* `--json` — Emit the JSON wire payload instead of the human-readable render
* `--check <NAME>` — Run only the named check(s). Repeatable. Defaults to all



## `bestool tamanu download`

Download Tamanu artifacts.

Use the `tamanu artifacts` subcommand to list of the artifacts available for a version.

Aliases: d, down

**Usage:** `bestool tamanu download [OPTIONS] <ARTIFACT TYPE> <VERSION>`

###### **Arguments:**

* `<ARTIFACT TYPE>` — Artifact type to download.

   You can find the artifact list using the `tamanu artifacts` subcommand.

   For backward compatibility, `web` is an alias to `frontend`, and `facility-server` / `central-server` are aliases to `facility` / `central`. Prefer the literal values.
* `<VERSION>` — Version to download

###### **Options:**

* `--into <INTO>` — Where to download to

  Default value: `.`
* `--url-only` — Print the URL, don't download.

   Useful if you want to download it on a different machine, or with a different tool.
* `--no-extract` — Don't extract (if the download is an archive)
* `-p`, `--platform <PLATFORM>` — Platform to download artifacts for.

   Use `host` (default) for the auto-detected current platform, `container` for container artifacts, `os-arch` for specific targets (e.g., `linux-x86_64`), and `all` to list all platforms.

   This is mostly useful with `--url-only` or `--no-extract`.

  Default value: `host`



## `bestool tamanu find`

Find Tamanu installations

**Usage:** `bestool tamanu find [OPTIONS]`

###### **Options:**

* `-n`, `--count <COUNT>` — Return this many entries
* `--asc` — Sort ascending
* `--with-version` — With version.

   Print parsed version information for each root.



## `bestool tamanu greenmask-config`

Generate a Greenmask config file

**Usage:** `bestool tamanu greenmask-config [OPTIONS] [FOLDERS]...`

###### **Arguments:**

* `<FOLDERS>` — Folders containing table masking definitions.

   Can be specified multiple times, entries will be merged.

   By default, it will look in the `greenmask/config` folder in the Tamanu root, and the `greenmask` folder in the Tamanu release folder. Non-existent folders are ignored.

###### **Options:**

* `--storage-dir <STORAGE_DIR>` — Folder where dumps are stored.

   By default, this is the `greenmask/dumps` folder in the Tamanu root.

   If the folder does not exist, it will be created.



## `bestool tamanu logs`

Tail logs for tamanu services and (optionally) caddy.

Each NAME is matched as a substring against the expected-Up service
list, so `tamanu logs api` picks up `tamanu-{central,facility}-api@*`
on systemd and `tamanu-api` on pm2. Multiple names combine: `tamanu
logs api fhir` tails both. With no names at all, every expected-Up
tamanu service is tailed alongside caddy.

The literal name `caddy` is recognised as a pseudo-service that
tails caddy: from `journalctl -u caddy.service` on Linux, and from
`.log` files under `C:\Caddy\logs` (or `C:\Caddy`) on Windows. Caddy
emits JSON-per-line logs; bestool detects these and applies
opportunistic syntax highlighting per line.

**Usage:** `bestool tamanu logs [OPTIONS] [NAMES]...`

###### **Arguments:**

* `<NAMES>` — Service names. Each is matched as a substring against the expected service list. `caddy` is a recognised pseudo-service. With no names, tails everything (every expected-Up tamanu service plus caddy)

###### **Options:**

* `-n`, `--lines <LINES>` — Number of trailing lines to print before tailing

  Default value: `10`
* `-f`, `--follow` — Follow: keep printing new lines as they arrive. Equivalent to `tail -f`
* `-g`, `--grep <REGEX>` — Only print lines matching this regex. On Linux this is passed to `journalctl -g`; on Windows it's applied client-side after reading from the log files



## `bestool tamanu meta-ticket`

Generate a meta-ticket for this Tamanu server

Connects to the Tamanu database, retrieves the device key, and produces a base64-encoded JSON ticket containing server identity information.

**Usage:** `bestool tamanu meta-ticket`



## `bestool tamanu psql`

Connect to Tamanu's database.

Aliases: p, pg, sql

**Usage:** `bestool tamanu psql [OPTIONS] [URL]`

###### **Arguments:**

* `<URL>` — Connect to postgres with a connection URL.

   This bypasses the discovery of credentials from Tamanu.

###### **Options:**

* `-U`, `--username <USERNAME>` — Connect to postgres with a different username.

   This may prompt for a password depending on your local settings and pg_hba config.
* `--ssl <SSL>` — SSL mode for the connection.

   Defaults to 'prefer' which attempts SSL but falls back to non-SSL. Use 'disable' to skip SSL entirely (useful on Windows with certificate issues). Use 'require' to enforce SSL connections.

   Ignored if a database URL is provided and it contains an sslmode parameter.

  Default value: `prefer`

  Possible values:
  - `disable`:
    Disable SSL/TLS encryption
  - `prefer`:
    Prefer SSL/TLS but allow unencrypted connections
  - `require`:
    Require SSL/TLS encryption

* `-W`, `--write` — Enable write mode for this psql.

   By default we set `TRANSACTION READ ONLY` for the session, which prevents writes. To enable writes, either pass this flag, or call `\W` within the session.

   This also disables autocommit, so you need to issue a COMMIT; command whenever you perform a write (insert, update, etc), as an extra safety measure.

   Additionally, enabling write mode will prompt for an OTS value. This should be the name of a person supervising the write operation, or a short message describing why you don't need one, such as "demo" or "emergency".
* `--theme <THEME>` — Syntax highlighting theme (light, dark, or auto)

   Controls the color scheme for SQL syntax highlighting in the input line. 'auto' attempts to detect terminal background, defaults to 'dark' if detection fails.

  Default value: `auto`

  Possible values:
  - `light`
  - `dark`
  - `auto`:
    Auto-detect terminal theme

* `--audit-path <PATH>` — Path to audit database directory (default: ~/.local/state/bestool-psql)
* `--no-redact` — Don't redact data

   This will also skip loading redactions.



## `bestool tamanu restart`

Rolling-restart all running tamanu services.

Background services (tasks, sync, fhir-*) restart in a single bulk
supervisor call. Critical services (api, frontend) restart one
instance at a time, each followed by a readiness probe, caddy
reload, and a cooldown — so there's always at least one critical
instance up to take traffic.

**Usage:** `bestool tamanu restart [OPTIONS] [NAMES]...`

###### **Arguments:**

* `<NAMES>` — Limit to expectations whose name contains any of these substrings. No names = restart every running instance of every Up expectation

###### **Options:**

* `--cooldown <COOLDOWN>` — Sleep between each critical-instance roll. Lets the fresh container settle and downstream caches warm up before we move on to the next one

  Default value: `30s`
* `--no-probe-http` — Skip the per-instance HTTP probe. Useful if the deployment isn't behind caddy (so the netavark IP doesn't matter) or you just want a fast best-effort restart without waiting on container readiness
* `--check-url <URL>` — After the rolling restart, hit this URL once to confirm end-to-end reachability. Bails non-zero if the probe fails



## `bestool tamanu start`

Bring up any expected tamanu services that aren't running.

Idempotent: services already up are left alone. Use `tamanu status`
first if you want to see what's missing.

**Usage:** `bestool tamanu start [NAMES]...`

###### **Arguments:**

* `<NAMES>` — Limit to expectations whose name contains any of these substrings. No names = start every Up expectation that's currently short



## `bestool tamanu status`

Report on tamanu services: what's expected vs what's actually running.

A lighter cousin of `tamanu doctor`: discovery only, no HTTP probes or
database queries. Useful as a quick "is anything down right now?"
check, or before/after a `tamanu start` / `restart` to see the impact.

**Usage:** `bestool tamanu status [OPTIONS] [NAMES]...`

###### **Arguments:**

* `<NAMES>` — Limit to expectations whose name contains any of these substrings. No names = report on every expectation

###### **Options:**

* `--json` — Emit the wire-shape JSON instead of the human-readable render



## `bestool tamanu stop`

Stop running tamanu services.

All matched services are stopped in a single supervisor call. Caddy
is not touched: its upstreams just become unreachable, which is
usually what's intended for a maintenance window.

**Usage:** `bestool tamanu stop [NAMES]...`

###### **Arguments:**

* `<NAMES>` — Limit to expectations whose name contains any of these substrings. No names = stop every running instance of every Up expectation



<hr/>

<small><i>
    This document was generated automatically by
    <a href="https://crates.io/crates/clap-markdown"><code>clap-markdown</code></a>.
</i></small>

