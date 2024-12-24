# Algae: simplified encryption commands.

You can install the CLI tool with:

```console
$ cargo install algae-cli
```

## Introduction

Algae is a simplified profile of the excellent [age](https://age-encryption.org/v1) format.

It implements five functions for the most common operations, and tries to be as obvious and
hard-to-misuse as possible, without being prohibitively hard to use, and while retaining
forward-compatibility with age (all algae products can be used with age, but not all age
products may be used with algae).

To start with, generate a keypair with `algae keygen`. This will generate two files:
`identity.txt.age`, a passphrase-protected keypair, and `identity.pub`, the public key in plain.

To encrypt a file, use `algae encrypt -k identity.pub filename`. As this uses the public key, it
doesn't require a passphrase. The encrypted file is written to `filename.age`. To decrypt it,
use `algae decrypt -k identity.txt.age filename.age`. As this uses the secret key, it will
prompt for its passphrase. The decoded file is written back to `filename` (i.e. without the
`.age` suffix).

To obtain a plaintext `identity.txt` (i.e. to remove the passphrase), use
`algae reveal identity.txt.age`. To add a new passphrase on a plaintext identity, use
`algae protect identity.txt`. These commands are not special to identity files: you can
`protect` (encrypt) and `reveal` (decrypt) arbitrary files with a passphrase.

## Library interface

Algae has a library interface ([a Rust crate](https://docs.rs/algae-cli)). It is peculiar in that it
deliberately exposes the CLI support structures alongside more traditional library routines, for the
purpose of embedding part or parcel of the Algae command set or conventions into other tools.

Documentation: https://docs.rs/algae-cli

## Name

_age_ is pronounced ah-gay. While [age doesn't have an inherent meaning](https://github.com/FiloSottile/age/discussions/329),
the Italian-adjacent Friulian language (spoken around Venice) word _aghe_, pronounced the same, means water.

Algae (pronounced al-gay or al-ghee) is **a** **l**ightweight (a)**ge**. Algae are also fond of water.

## License

The tool and library are licensed GPL-3.0 or later.
