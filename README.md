# bestool

All-in-one tool for BES ops and dev tasks.

It manifests as a single binary that can be easily uploaded to Windows machines, or downloaded from the internet, and also works cross-platform on Linux and Mac for many tasks.

See `bestool <subcommand> --help` for extensive help.

## Download

| Platform | Variant | Download |
| -------- | ------- | -------- |
| Windows | x86 | [bestool.exe](https://tools.ops.tamanu.io/bestool/latest/x86_64-pc-windows-msvc/bestool.exe) |
| Linux | x86 | [bestool](https://tools.ops.tamanu.io/bestool/latest/x86_64-unknown-linux-gnu/bestool) |
| Linux | x86 static | [bestool](https://tools.ops.tamanu.io/bestool/latest/x86_64-unknown-linux-musl/bestool) |
| Linux | ARM64 | [bestool](https://tools.ops.tamanu.io/bestool/latest/aarch64-unknown-linux-musl/bestool) |
| Mac | Intel | [bestool](https://tools.ops.tamanu.io/bestool/latest/x86_64-apple-darwin/bestool) |
| Mac | ARM64 | [bestool](https://tools.ops.tamanu.io/bestool/latest/aarch64-apple-darwin/bestool) |

These URLs always point to the latest release. Pin to a specific version with `https://tools.ops.tamanu.io/bestool/<version>/<target>/bestool`.

### Self-update

If you already have bestool, it can self-update to the latest version:

```console
$ bestool self-update
```

### APT repository

If you're on Debian or a derivative, you can use our APT repo:

```bash
curl -fsSL https://tools.ops.tamanu.io/apt/bes-tools.gpg.key | sudo gpg --dearmor -o /etc/apt/keyrings/bes-tools.gpg
echo "deb [signed-by=/etc/apt/keyrings/bes-tools.gpg] https://tools.ops.tamanu.io/apt stable main" | sudo tee /etc/apt/sources.list.d/bes-tools.list
sudo apt-get update
sudo apt-get install bestool
```

### In GitHub Actions

```yaml
- name: Download bestool
  shell: bash
  run: |
    curl -Lo ${{ runner.os == 'Windows' && 'bestool.exe' || 'bestool' }} https://tools.ops.tamanu.io/bestool/gha/${{ runner.os }}-${{ runner.arch }}?bust=${{ github.run_id }}
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
    curl -Lo $bestool https://tools.ops.tamanu.io/bestool/gha/${{ runner.os }}-${{ runner.arch }}?bust=${{ github.run_id }}
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

### Releasing

Releases are automated by [release-plz](https://release-plz.dev). Pushing to `main` opens a `repo: release` PR with per-crate version bumps determined from conventional commits and `cargo-semver-checks`; merging that PR (auto-merge is enabled once CI is green) publishes the affected crates to crates.io, pushes per-crate tags, and triggers the binary-build workflows.

#### Holding a release

To bundle several features or fixes into one release, add the `release-hold` label to the open `repo: release` PR. The CI turns auto-merge off when the label is added and won't turn it back on while the label is present, so subsequent merges to `main` keep updating the PR without shipping it. When you're ready to ship, remove the label and either queue the PR manually or push another commit to `main` to let the workflow re-enable auto-merge. (Don't rely on removing the `autorelease` label; release-plz re-adds it whenever it updates the PR.)
