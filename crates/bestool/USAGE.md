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
* [`bestool self-update`↴](#bestool-self-update)
* [`bestool ssh`↴](#bestool-ssh)
* [`bestool ssh add-key`↴](#bestool-ssh-add-key)
* [`bestool tamanu`↴](#bestool-tamanu)
* [`bestool tamanu alerts`↴](#bestool-tamanu-alerts)
* [`bestool tamanu alertd`↴](#bestool-tamanu-alertd)
* [`bestool tamanu alertd run`↴](#bestool-tamanu-alertd-run)
* [`bestool tamanu alertd reload`↴](#bestool-tamanu-alertd-reload)
* [`bestool tamanu alertd loaded-alerts`↴](#bestool-tamanu-alertd-loaded-alerts)
* [`bestool tamanu alertd pause-alert`↴](#bestool-tamanu-alertd-pause-alert)
* [`bestool tamanu alertd validate`↴](#bestool-tamanu-alertd-validate)
* [`bestool tamanu artifacts`↴](#bestool-tamanu-artifacts)
* [`bestool tamanu backup`↴](#bestool-tamanu-backup)
* [`bestool tamanu backup-configs`↴](#bestool-tamanu-backup-configs)
* [`bestool tamanu config`↴](#bestool-tamanu-config)
* [`bestool tamanu db-url`↴](#bestool-tamanu-db-url)
* [`bestool tamanu download`↴](#bestool-tamanu-download)
* [`bestool tamanu find`↴](#bestool-tamanu-find)
* [`bestool tamanu greenmask-config`↴](#bestool-tamanu-greenmask-config)
* [`bestool tamanu psql`↴](#bestool-tamanu-psql)

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

   If the path provided is a directory, a file will be created in that directory. The file name will be the current date and time, in the format `programname.YYYY-MM-DDTHH-MM-SSZ.log`.
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
* `download` — Download Tamanu artifacts
* `find` — Find Tamanu installations
* `greenmask-config` — Generate a Greenmask config file
* `psql` — Connect to Tamanu's database

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
* `reload` — Send reload signal to running daemon
* `loaded-alerts` — List currently loaded alert files
* `pause-alert` — Temporarily pause an alert
* `validate` — Validate an alert definition file



## `bestool tamanu alertd run`

Run the alert daemon

Starts the daemon which monitors alert definition files and executes alerts based on their configured schedules. The daemon will watch for file changes and automatically reload when definitions are modified.

**Usage:** `bestool tamanu alertd run [OPTIONS]`

###### **Options:**

* `--dir <DIR>` — Glob patterns for alert definitions

   Patterns can match directories (which will be read recursively) or individual files. Can be provided multiple times. Examples: /etc/tamanu/alerts, /opt/*/alerts, /etc/tamanu/alerts/**/*.yml
* `--dry-run` — Execute all alerts once and quit (ignoring intervals)
* `--no-server` — Disable the HTTP server
* `--server-addr <SERVER_ADDR>` — HTTP server bind address(es)

   Can be provided multiple times. The server will attempt to bind to each address in order until one succeeds. Defaults to [::1]:8271 and 127.0.0.1:8271



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



<hr/>

<small><i>
    This document was generated automatically by
    <a href="https://crates.io/crates/clap-markdown"><code>clap-markdown</code></a>.
</i></small>

