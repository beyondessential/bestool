# bestool

All-in-one tool for BES ops and dev tasks.

It manifests as a single binary that can be easily uploaded to Windows machines, or downloaded from the internet, and also works cross-platform on Linux and Mac for many tasks.

See `bestool <subcommand> --help` for extensive help.

## Download

Current release: 0.9.0

| Platform | Variant | Download |
| -------- | ------- | -------- |
| Windows | x86 | [bestool.exe](https://tools.ops.tamanu.io/bestool/0.9.0/x86_64-pc-windows-msvc/bestool.exe) |
| Linux | x86 | [bestool](https://tools.ops.tamanu.io/bestool/0.9.0/x86_64-unknown-linux-gnu/bestool) |
| Linux | x86 static | [bestool](https://tools.ops.tamanu.io/bestool/0.9.0/x86_64-unknown-linux-musl/bestool) |
| Linux | ARM64 | [bestool](https://tools.ops.tamanu.io/bestool/0.9.0/aarch64-unknown-linux-musl/bestool) |
| Mac | Intel | [bestool](https://tools.ops.tamanu.io/bestool/0.9.0/x86_64-apple-darwin/bestool) |
| Mac | ARM64 | [bestool](https://tools.ops.tamanu.io/bestool/0.9.0/aarch64-apple-darwin/bestool) |

### Self-update

If you already have bestool, it can self-update to the latest version:

```console
$ bestool self-update
```

### Always-latest URLs

The above URLs are for the current release. If you want to always get the latest version, you can use the following URLs:

| Platform | Variant | Download |
| -------- | ------- | -------- |
| Windows | x86 | [bestool.exe](https://tools.ops.tamanu.io/bestool/latest/x86_64-pc-windows-msvc/bestool.exe) |
| Linux | x86 | [bestool](https://tools.ops.tamanu.io/bestool/latest/x86_64-unknown-linux-gnu/bestool) |
| Linux | x86 static | [bestool](https://tools.ops.tamanu.io/bestool/latest/x86_64-unknown-linux-musl/bestool) |
| Linux | ARM64 | [bestool](https://tools.ops.tamanu.io/bestool/latest/aarch64-unknown-linux-musl/bestool) |
| Mac | Intel | [bestool](https://tools.ops.tamanu.io/bestool/latest/x86_64-apple-darwin/bestool) |
| Mac | ARM64 | [bestool](https://tools.ops.tamanu.io/bestool/latest/aarch64-apple-darwin/bestool) |

### In GitHub Actions

```yaml
- name: Download bestool
  shell: bash
  run: |
    curl -Lo ${{ runner.os == 'Windows' && 'bestool.exe' || 'bestool' }} https://tools.ops.tamanu.io/bestool/gha/${{ runner.os }}-${{ runner.arch }}
    [[ -f bestool ]] && chmod +x bestool

- name: Use bestool
  shell: bash
  run: |
    bestool=bestool
    [[ ${{ runner.os }} == "Windows" ]] && bestool=bestool.exe
    ./$bestool --version # or something more useful
```

Or combined:

```yaml
- name: Download bestool
  shell: bash
  run: |
    bestool=bestool
    [[ ${{ runner.os }} == "Windows" ]] && bestool=bestool.exe
    curl -Lo $bestool https://tools.ops.tamanu.io/bestool/gha/${{ runner.os }}-${{ runner.arch }}
    [[ -f bestool ]] && chmod +x bestool
    ./$bestool --version # or something more useful
```

### With [Binstall](https://github.com/cargo-bins/cargo-binstall)

```console
$ cargo binstall bestool
```

### With cargo (compiling)

```console
$ cargo install bestool
```

## Development

Install [rust](https://rustup.rs), clone the repo, then run:

```console
$ cargo check
```

To run the tool:

```console
$ cargo run -- --help
```

To build the tool like for production:

```console
$ cargo build --release
```

Commits should follow the [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/) format.
Types are listed in the [cliff.toml](./cliff.toml#L62-L78) file.

### Releasing

To make a release, install [cargo-release](https://github.com/crate-ci/cargo-release) and [git-cliff](https://git-cliff.org/), then:

```console
$ git switch main
$ git pull
$ cargo release minor --execute
```

(or `patch` or `major` instead of `minor`)
