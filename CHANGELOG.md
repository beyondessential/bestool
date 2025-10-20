# Changelog

All notable changes to this project will be documented in this file.

See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

---
## [1.0.0](https://github.com/beyondessential/bestool/compare/v0.30.3..v1.0.0) - 2025-10-20


- **Bugfix:** Codepage setting - ([44f0317](https://github.com/beyondessential/bestool/commit/44f03172da24129b82dc5151b7776cd7206ba7ce))
- **Bugfix:** Default-run - ([eead678](https://github.com/beyondessential/bestool/commit/eead6781887e25a493439ab2b3ea8c97f59c67d4))
- **Deps:** Upgrade psql deps - ([2575068](https://github.com/beyondessential/bestool/commit/2575068d9da8e8bd585d4fb6b5d2c4eb9079111c))
- **Deps:** Fix optional deps - ([d1c79ea](https://github.com/beyondessential/bestool/commit/d1c79ea929bb6140c8a73856a400263f9a9b3213))
- **Style:** Fix clippy - ([a5a571f](https://github.com/beyondessential/bestool/commit/a5a571f9d27177941978f934a8f508991918a344))
- **Style:** Use proper types and traits - ([30ce373](https://github.com/beyondessential/bestool/commit/30ce3734bf8206a2723f4f1ec82c1004235b8a32))

### Psql

- **Feature:** Syntax highlighting - ([67e93d4](https://github.com/beyondessential/bestool/commit/67e93d4c10f3aad2aeb034136ecea5e72da6d186))

### Tamanu

- **Feature:** Use new psql tool - ([fb5b6ae](https://github.com/beyondessential/bestool/commit/fb5b6ae49dbdb5b9093cb6b2810a19aab0bab777))

---
## [0.30.3](https://github.com/beyondessential/bestool/compare/v0.30.2..v0.30.3) - 2025-10-09


- **Documentation:** Document aliases - ([4308848](https://github.com/beyondessential/bestool/commit/430884834f42a078bf200bb49a3d8672c23ccb78))

---
## [0.30.2](https://github.com/beyondessential/bestool/compare/v0.30.1..v0.30.2) - 2025-10-09



### Db-url

- **Bugfix:** Handle the case where a reporting username is empty in config - ([8af528a](https://github.com/beyondessential/bestool/commit/8af528a6a71e2630a10b154d77aad7c4e11f6fd5))

---
## [0.30.1](https://github.com/beyondessential/bestool/compare/v0.30.0..v0.30.1) - 2025-10-09


- **Tweak:** Use hand-picked short aliases instead of inferred shorthands - ([1f1312e](https://github.com/beyondessential/bestool/commit/1f1312e9b4afff7135de87de7251a2f0c6643588))

---
## [0.30.0](https://github.com/beyondessential/bestool/compare/v0.29.2..v0.30.0) - 2025-10-09


- **Deps:** Bump the deps group across 1 directory with 32 updates (#217) - ([d8693fc](https://github.com/beyondessential/bestool/commit/d8693fc00b4cb98a269488f0d3c4cf4a89646015))
- **Deps:** Bump the deps group with 8 updates (#218) - ([d4003b2](https://github.com/beyondessential/bestool/commit/d4003b2b9fcbeaf73ce6994c1f146c56c8da154b))
- **Deps:** Bump the deps group with 6 updates (#220) - ([93e8e9f](https://github.com/beyondessential/bestool/commit/93e8e9f86ff28f0d158f3891d850ab01eba7dc90))
- **Deps:** Bump the deps group with 6 updates (#222) - ([17c8340](https://github.com/beyondessential/bestool/commit/17c83404bec0be56929530e6e6457d653e2e74a9))

### Psql

- **Tweak:** Use reporting users when present - ([0bcb91f](https://github.com/beyondessential/bestool/commit/0bcb91f10b457aa9bc5ae7d12ad66acb12511542))
- **Tweak:** Allow customising the codepage on windows - ([ffd3aff](https://github.com/beyondessential/bestool/commit/ffd3aff57c5b26450c68662b3a4ec47ceb15ab5c))

### Tamanu

- **Feature:** Add dburl command - ([7eb2c90](https://github.com/beyondessential/bestool/commit/7eb2c9079d7393d6ee2b09f0d95b880e927e0ef6))
- **Tweak:** Support port field - ([2407714](https://github.com/beyondessential/bestool/commit/24077145d5ff64153fd7abc83a378efc5913b79d))
- **Tweak:** Move dburl command to url for mnemonics - ([e38cfcc](https://github.com/beyondessential/bestool/commit/e38cfccc0dae4b30f371c56a3306a6a1687e8b2e))

### Url

- **Tweak:** Don't include empty password if no password is provided - ([9117dd5](https://github.com/beyondessential/bestool/commit/9117dd5a32231002f00c577b367ad1deaa3185ac))

---
## [0.29.2](https://github.com/beyondessential/bestool/compare/v0.29.1..v0.29.2) - 2025-07-29



### Psql

- **Tweak:** Default to \timing on - ([68843f2](https://github.com/beyondessential/bestool/commit/68843f2ab3c74acb7c1abce83e805ea8aad5df8b))

---
## [0.29.1](https://github.com/beyondessential/bestool/compare/v0.29.0..v0.29.1) - 2025-07-28



### Psql

- **Tweak:** Use UTF-8 codepage on Windows and force UTF8 encoding on PSQL - ([797fb83](https://github.com/beyondessential/bestool/commit/797fb835e66c5522f0c111aaa292474c4b253b44))

---
## [0.29.0](https://github.com/beyondessential/bestool/compare/v0.28.5..v0.29.0) - 2025-07-09



### Tamanu

- **Feature:** Command to list artifacts from meta (#212) - ([d5ead2e](https://github.com/beyondessential/bestool/commit/d5ead2e87f7b0abbfac4fa4f1e55268d9cc6bcf1))
- **Feature:** Remove unused upgrade and pre-upgrade commands - ([d5ead2e](https://github.com/beyondessential/bestool/commit/d5ead2e87f7b0abbfac4fa4f1e55268d9cc6bcf1))
- **Tweak:** Make download command able to download any artifact - ([d5ead2e](https://github.com/beyondessential/bestool/commit/d5ead2e87f7b0abbfac4fa4f1e55268d9cc6bcf1))

---
## [0.28.5](https://github.com/beyondessential/bestool/compare/v0.28.4..v0.28.5) - 2025-06-10



### Cli

- **Feature:** Enable unambiguous shorthands - ([312cca9](https://github.com/beyondessential/bestool/commit/312cca9dada2c769235758c8efa6767b3fd2eca7))

---
## [0.28.4](https://github.com/beyondessential/bestool/compare/v0.28.3..v0.28.4) - 2025-06-08



### Backups

- **Tweak:** Only exclude non-critical log tables (#210) - ([6447300](https://github.com/beyondessential/bestool/commit/6447300e3030c2809a7aa51e657f3c7f7c971edb))

---
## [0.28.3](https://github.com/beyondessential/bestool/compare/v0.28.2..v0.28.3) - 2025-03-18


- **Deps:** Bump the deps group with 11 updates (#197) - ([2d6e431](https://github.com/beyondessential/bestool/commit/2d6e431759cf24a492881cc3ce0d62bc1c473105))

---
## [0.28.2](https://github.com/beyondessential/bestool/compare/v0.28.1..v0.28.2) - 2025-03-14


- **Deps:** Bump the deps group with 7 updates (#196) - ([bbdf6c0](https://github.com/beyondessential/bestool/commit/bbdf6c0cfcaa489684aefca3757b7fcb568eb668))

### Backups

- **Bugfix:** Don’t nest backup in duplicate folders when splitting - ([0d3abe0](https://github.com/beyondessential/bestool/commit/0d3abe057898ab01b5b21d39e4bf08828fc9bd69))

---
## [0.28.1](https://github.com/beyondessential/bestool/compare/v0.28.0..v0.28.1) - 2025-03-06


- **Repo:** Add walg feature back so builds don’t break - ([f5908df](https://github.com/beyondessential/bestool/commit/f5908dfb863bbd7327b82dabc1e4e83ad3e934e6))

---
## [0.28.0](https://github.com/beyondessential/bestool/compare/v0.27.0..v0.28.0) - 2025-03-06


- **Repo:** Completely remove dyndns - ([8bacd55](https://github.com/beyondessential/bestool/commit/8bacd55cacd14315599e69a3875543ce3262a7d9))
- **Repo:** Remove useless file_chunker - ([16d55f5](https://github.com/beyondessential/bestool/commit/16d55f556b0a171b56f80e4bcfa8eccf07266955))

### Psql

- **Feature:** Arbitrary program and args - ([efed90a](https://github.com/beyondessential/bestool/commit/efed90a4431058e0efe230f43b88ee883a4f160e))
- **Tweak:** Turn autocommit off when -W is given - ([b7636ce](https://github.com/beyondessential/bestool/commit/b7636ce3b197fb2301d329a9ce74db90ccfb3eb8))

---
## [0.27.0](https://github.com/beyondessential/bestool/compare/v0.26.7..v0.27.0) - 2025-03-05


- **Feature:** KAM-341: split and join files (with backup support) (#194) - ([ea3e9f9](https://github.com/beyondessential/bestool/commit/ea3e9f9737f1db5460e8666a890e44c212524bc0))

---
## [0.26.7](https://github.com/beyondessential/bestool/compare/v0.26.6..v0.26.7) - 2025-03-04


- **Deps:** Bump the deps group across 1 directory with 14 updates (#188) - ([650984b](https://github.com/beyondessential/bestool/commit/650984b23fb7bc6bb12b722489f10588d2747c62))
- **Deps:** Bump the deps group across 1 directory with 15 updates (#193) - ([751581d](https://github.com/beyondessential/bestool/commit/751581df704570d3eaa57ef1a64f8da152fb2d79))

### Tamanu

- **Bugfix:** Do not require mailgun until needed - ([99496d2](https://github.com/beyondessential/bestool/commit/99496d258b921892c1fcf439bd55275ecf01057d))

---
## [0.26.5](https://github.com/beyondessential/bestool/compare/v0.26.4..v0.26.5) - 2025-02-04


- **Deps:** Bump the deps group across 1 directory with 9 updates (#186) - ([706ea9d](https://github.com/beyondessential/bestool/commit/706ea9dc8e6958e40b24e7a5efff18d1f1fd9896))
- **Repo:** Remove dyndns feature - ([6e33015](https://github.com/beyondessential/bestool/commit/6e33015d33b66bcfcdc5321209442bbd4da78797))

### Downloads

- **Bugfix:** Query tailscale dns directly to avoid buggy systems - ([63dccad](https://github.com/beyondessential/bestool/commit/63dccadcd8a2b2efe6ee14e57d19e356e0ef59eb))

---
## [0.26.4](https://github.com/beyondessential/bestool/compare/v0.26.3..v0.26.4) - 2025-02-04



### Downloads

- **Bugfix:** Use full tailscale name for alternative sources - ([ebb6522](https://github.com/beyondessential/bestool/commit/ebb6522d90c0af3837f2b166c0524a0434457f46))

---
## [0.26.3](https://github.com/beyondessential/bestool/compare/v0.26.2..v0.26.3) - 2025-02-04


- **Bugfix:** Whoops windows things again - ([c632a00](https://github.com/beyondessential/bestool/commit/c632a00457805a1e54233de75608ecf24ed48eae))

---
## [0.26.2](https://github.com/beyondessential/bestool/compare/v0.26.1..v0.26.2) - 2025-02-04


- **Bugfix:** Whoops extraneous `async` - ([96ccc26](https://github.com/beyondessential/bestool/commit/96ccc2683a2da3914044a0af0b44aee6bedbfe6b))

---
## [0.26.1](https://github.com/beyondessential/bestool/compare/v0.26.0..v0.26.1) - 2025-02-04


- **Refactor:** Use lloggs instead of custom logging code - ([0297fdc](https://github.com/beyondessential/bestool/commit/0297fdc3bb30584e3cb28effdf3645f8a3b5197a))
- **Repo:** Temporarily disable publishing to crates.io - ([8e8dd29](https://github.com/beyondessential/bestool/commit/8e8dd29c10706be45a4f5712b81be859d22c1f13))

### Bestool

- **Feature:** Download from tailscale proxies when available - ([c8fa0ab](https://github.com/beyondessential/bestool/commit/c8fa0abd8ae498fb090a4b8bcc840847b6838793))

---
## [0.26.0](https://github.com/beyondessential/bestool/compare/v0.25.8..v0.26.0) - 2025-01-29


- **Bugfix:** Fix tests - ([e94131c](https://github.com/beyondessential/bestool/commit/e94131cc54096669b7692f68253ebba6b8e1ad49))
- **Bugfix:** Fix more tests - ([65951f8](https://github.com/beyondessential/bestool/commit/65951f8ff0b4435051094dabda4ad67a2347ccfe))

### Alerts

- **Feature:** Render email html body from markdown - ([d4bb1e8](https://github.com/beyondessential/bestool/commit/d4bb1e8915aae77d3e04caf80d2997b48fac0f29))
- **Refactor:** Split into mods - ([af98c55](https://github.com/beyondessential/bestool/commit/af98c555632f9a8370b2de2982cb3944ed58fbcd))
- **Refactor:** Split alerts into mods - ([f7a5407](https://github.com/beyondessential/bestool/commit/f7a54070dd0c563f436022dcbf53eb1a08f7353a))
- **Test:** Remove legacy alert definition support - ([155e79b](https://github.com/beyondessential/bestool/commit/155e79bb7c03debe494afc46d80dbe6e1be08e08))

---
## [0.25.8](https://github.com/beyondessential/bestool/compare/v0.25.7..v0.25.8) - 2025-01-28



### Alerts

- **Tweak:** Print which folders are searched - ([2916298](https://github.com/beyondessential/bestool/commit/29162988784b913a462ce89fe7c9f0d647f77723))

---
## [0.25.7](https://github.com/beyondessential/bestool/compare/v0.25.6..v0.25.7) - 2025-01-28



### Alerts

- **Tweak:** Cover more default dirs (toolbox container, cwd) - ([7a2a582](https://github.com/beyondessential/bestool/commit/7a2a5826d646599f4b31168bb412ea1a4dfec8d0))

---
## [0.25.6](https://github.com/beyondessential/bestool/compare/v0.25.5..v0.25.6) - 2025-01-28


- **Deps:** Make html2md conditional - ([a0af4fa](https://github.com/beyondessential/bestool/commit/a0af4fa054a519f1c70e47773516628acb4a379c))

### Alerts

- **Feature:** Default to reading from the right places - ([b4834f7](https://github.com/beyondessential/bestool/commit/b4834f7ce81dbb1132f9c153763d52e4e2724a60))

---
## [0.25.5](https://github.com/beyondessential/bestool/compare/v0.25.4..v0.25.5) - 2025-01-28



### Alerts

- **Feature:** Render slack alerts to markdown if they’re html - ([110a90a](https://github.com/beyondessential/bestool/commit/110a90ae88e5fadcfc049a97aa58e333876c854b))

---
## [0.25.4](https://github.com/beyondessential/bestool/compare/v0.25.3..v0.25.4) - 2025-01-28



### Alerts

- **Bugfix:** Specify which alert timed out - ([c563b1e](https://github.com/beyondessential/bestool/commit/c563b1e5d5632c909e51e50673165c5c5ea0f53b))

---
## [0.25.3](https://github.com/beyondessential/bestool/compare/v0.25.2..v0.25.3) - 2025-01-28



### Alerts

- **Bugfix:** Report on timeouts - ([97fe962](https://github.com/beyondessential/bestool/commit/97fe9627e814f6af36867916c15d6144f8fb606e))

---
## [0.25.2](https://github.com/beyondessential/bestool/compare/v0.25.1..v0.25.2) - 2025-01-28



### Alerts

- **Feature:** Add --timeout for alerts to avoid blocking indefinitely - ([889301c](https://github.com/beyondessential/bestool/commit/889301cbac2fb1ac494c9bbe1db5009696216994))
- **Performance:** Run alerts in parallel - ([4246c11](https://github.com/beyondessential/bestool/commit/4246c1114ead79edffaef201c8acde605e786fc8))

---
## [0.25.1](https://github.com/beyondessential/bestool/compare/v0.25.0..v0.25.1) - 2025-01-28



### Backups

- **Tweak:** Use filesystem copy if we can - ([e96eec0](https://github.com/beyondessential/bestool/commit/e96eec03018271da06210694e512bd903152b3d2))

---
## [0.25.0](https://github.com/beyondessential/bestool/compare/v0.24.10..v0.25.0) - 2025-01-28


- **Bugfix:** Fix tests - ([7a383e5](https://github.com/beyondessential/bestool/commit/7a383e5132d506264d7f06d1332e427034affe7f))
- **Style:** Remove a warning - ([03932be](https://github.com/beyondessential/bestool/commit/03932be80b2dfc80eb7fcdf84923a88a92c5ae23))

### Alerts

- **Feature:** Add slack and multiplexed external targets - ([5573478](https://github.com/beyondessential/bestool/commit/5573478428760cc3642cf62cfd380aa175e3cbe4))

### Backups

- **Test:** Remove --deterministic - ([8f88348](https://github.com/beyondessential/bestool/commit/8f8834885d0d8adab8c47ab0babc099676530eae))
- **Tweak:** Use zero-compression zips - ([bc3e064](https://github.com/beyondessential/bestool/commit/bc3e0640473a18b994632b306117c75a3fc9b5c4))

---
## [0.24.10](https://github.com/beyondessential/bestool/compare/v0.24.9..v0.24.10) - 2025-01-24



### Backups

- **Feature:** Create dir for --then-copy-to - ([fbb72d5](https://github.com/beyondessential/bestool/commit/fbb72d5bf7705d154ddf7210abb8db8264a88a31))
- **Test:** Can't do deterministic zips - ([29a1f02](https://github.com/beyondessential/bestool/commit/29a1f0281b4135b99fb329e8855f1028ee66fa97))

---
## [0.24.9](https://github.com/beyondessential/bestool/compare/v0.24.8..v0.24.9) - 2025-01-24



### Backup

- **Test:** Update snapshot - ([4abcfc1](https://github.com/beyondessential/bestool/commit/4abcfc10238e83f940a4e837f3d752d6f18a04b1))

### Backups

- **Feature:** Create dest dir and fix log output - ([f5794c2](https://github.com/beyondessential/bestool/commit/f5794c25f8712e267ff687570a906c8ab05c3df2))
- **Feature:** Use zip for configs instead of tar - ([3bc7852](https://github.com/beyondessential/bestool/commit/3bc7852c13e4880a0abf70cc1459d857de1390ba))

---
## [0.24.8](https://github.com/beyondessential/bestool/compare/v0.24.7..v0.24.8) - 2025-01-24


- **Deps:** Bump the deps group with 14 updates (#180) - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **Deps:** Upgrade clap to 4.5.26 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **Deps:** Upgrade jiff to 0.1.22 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **Deps:** Upgrade tokio to 1.43.0 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **Deps:** Upgrade aws-sdk-route53 to 1.58.0 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **Deps:** Upgrade aws-sdk-sts to 1.54.1 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **Deps:** Upgrade binstalk-downloader to 0.13.8 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **Deps:** Upgrade bitflags to 2.7.0 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **Deps:** Upgrade clap_complete to 4.5.42 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **Deps:** Upgrade detect-targets to 0.1.36 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **Deps:** Upgrade dirs to 6.0.0 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **Deps:** Upgrade serde_json to 1.0.135 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **Deps:** Upgrade thiserror to 2.0.11 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **Deps:** Upgrade uuid to 1.11.1 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **Deps:** Upgrade windows to 0.59.0 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))

### Backups

- **Feature:** Add encryption and --then-copy-to and --keep-days to config backups - ([7c7b66e](https://github.com/beyondessential/bestool/commit/7c7b66e8760c790efcf1075c77b618f4c74a3e6b))

---
## [0.24.7](https://github.com/beyondessential/bestool/compare/v0.24.6..v0.24.7) - 2025-01-09



### Backups

- **Bugfix:** Compute output filename properly - ([10e6bae](https://github.com/beyondessential/bestool/commit/10e6baeb4b3d1eac100d50b34e5a8f48d5f793bc))

---
## [0.24.6](https://github.com/beyondessential/bestool/compare/v0.24.5..v0.24.6) - 2025-01-09



### Backups

- **Tweak:** Do file copy in Rust to get a progress indication - ([75ccf63](https://github.com/beyondessential/bestool/commit/75ccf6397a81adf7f066afd43c14345fadb14920))

---
## [0.24.5](https://github.com/beyondessential/bestool/compare/v0.24.4..v0.24.5) - 2025-01-09


- **Style:** Don't mix tokio and std io - ([bb6e07c](https://github.com/beyondessential/bestool/commit/bb6e07c9ef4a7b68b72592f9f04fad69135227b9))

---
## [0.24.0](https://github.com/beyondessential/bestool/compare/v0.23.3..v0.24.0) - 2025-01-06


- **Deps:** Bump the deps group with 10 updates (#176) - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **Deps:** Update jiff to 0.1.16 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **Deps:** Update aws-config to 1.5.12 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **Deps:** Update aws-sdk-route53 to 1.56.0 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **Deps:** Update aws-sdk-sts to 1.53.0 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **Deps:** Update boxcar to 0.2.8 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **Deps:** Update detect-targets to 0.1.34 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **Deps:** Update glob to 0.3.2 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **Deps:** Update reqwest to 0.12.11 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **Deps:** Update serde to 1.0.217 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **Deps:** Update sysinfo to 0.33.1 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **Deps:** Upgrade itertools to 0.14.0 - ([2c7e4c8](https://github.com/beyondessential/bestool/commit/2c7e4c8d132a72454fd74cb02a1b85ad586c12c4))

### Backups

- **Feature:** Add --keep-days option to cleanup old backups - ([232336b](https://github.com/beyondessential/bestool/commit/232336b2bb0ed46c6ac11f6aae26fd550f803076))

---
## [0.23.1](https://github.com/beyondessential/bestool/compare/v0.23.0..v0.23.1) - 2024-12-24


- **Repo:** Temporarily downgrade algae to 0.0.0 for release purposes - ([9d564c6](https://github.com/beyondessential/bestool/commit/9d564c6670af75f952c86733b908e8fd6ac3266a))

### Crypto

- **Refactor:** Use algae-cli in bestool - ([347af7a](https://github.com/beyondessential/bestool/commit/347af7ac6e05fdc50005abd5ca70eb5ae7a89a88))

---
## [0.23.0](https://github.com/beyondessential/bestool/compare/v0.22.0..v0.23.0) - 2024-12-23


- **Deps:** Bump the deps group with 3 updates (#173) - ([688a4f1](https://github.com/beyondessential/bestool/commit/688a4f1ba679bf78d8a86e98751a97a2a789b842))
- **Deps:** Update age to 0.11.1 - ([688a4f1](https://github.com/beyondessential/bestool/commit/688a4f1ba679bf78d8a86e98751a97a2a789b842))
- **Deps:** Update serde_json to 1.0.134 - ([688a4f1](https://github.com/beyondessential/bestool/commit/688a4f1ba679bf78d8a86e98751a97a2a789b842))
- **Deps:** Update thiserror to 2.0.9 - ([688a4f1](https://github.com/beyondessential/bestool/commit/688a4f1ba679bf78d8a86e98751a97a2a789b842))

### Backup

- **Documentation:** Fix help for trailing args - ([95df135](https://github.com/beyondessential/bestool/commit/95df1350df8b1d136f5cce9a45aa883b1c0951bc))
- **Feature:** KAM-297: add ability to encrypt backups (#174) - ([f7c77af](https://github.com/beyondessential/bestool/commit/f7c77afda4c586c9178d5d5c864582272238e65b))

### Crypto

- **Documentation:** Explain how to use the identity file in keygen - ([f7c77af](https://github.com/beyondessential/bestool/commit/f7c77afda4c586c9178d5d5c864582272238e65b))
- **Documentation:** Fix description of keygen - ([374d4ae](https://github.com/beyondessential/bestool/commit/374d4aeae0c6d4e7b7a600c9490399400c7c517e))
- **Feature:** Add protect/reveal commands for passphrase encryption - ([75f8e1d](https://github.com/beyondessential/bestool/commit/75f8e1d35aa01e27099822d0e77a96e75701317f))
- **Feature:** Encrypt identity files by default - ([84061c3](https://github.com/beyondessential/bestool/commit/84061c307c028996fb5222d696cc7f569687363c))
- **Feature:** Support encrypted identity files directly while en/decrypting - ([abc86a8](https://github.com/beyondessential/bestool/commit/abc86a8464820905a814215d9afa11b37b61eea6))
- **Feature:** Add --rm to encrypt and protect - ([8828421](https://github.com/beyondessential/bestool/commit/88284210a5fa0ad2dce97f208248b9ab4adbcc70))
- **Feature:** Write identity.pub by default - ([a39d39d](https://github.com/beyondessential/bestool/commit/a39d39d67f7e8ad7c7777cfc1732471c1ee249a9))
- **Refactor:** Extract en/decryption and key handling routines - ([f7c77af](https://github.com/beyondessential/bestool/commit/f7c77afda4c586c9178d5d5c864582272238e65b))

---
## [0.22.0](https://github.com/beyondessential/bestool/compare/v0.21.5..v0.22.0) - 2024-12-20


- **Deps:** Update rppal requirement from 0.20.0 to 0.22.1 in /crates/bestool in the deps group (#171) - ([1c4f2d1](https://github.com/beyondessential/bestool/commit/1c4f2d1a6c77f561e1a78dece5ebb8ed2336ba81))
- **Deps:** Bump the deps group across 1 directory with 10 updates (#172) - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))
- **Deps:** Update aws-config to 1.5.11 - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))
- **Deps:** Update aws-sdk-route53 to 1.55.0 - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))
- **Deps:** Update binstalk-downloader to 0.13.6 - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))
- **Deps:** Update chrono to 0.4.39 - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))
- **Deps:** Update clap_complete to 4.5.40 - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))
- **Deps:** Update detect-targets to 0.1.33 - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))
- **Deps:** Update rppal to 0.22.1 - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))
- **Deps:** Update serde to 1.0.216 - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))

### Crypto

- **Feature:** KAM-297: add encrypt, decrypt, and keygen (#169) - ([a5367c3](https://github.com/beyondessential/bestool/commit/a5367c3c239045ea09a4336e3308fbd64d1bcddf))

---
## [0.21.5](https://github.com/beyondessential/bestool/compare/v0.21.4..v0.21.5) - 2024-12-18


- **Deps:** Bump the deps group with 7 updates (#167) - ([7b365b1](https://github.com/beyondessential/bestool/commit/7b365b1d6208ab1f64847f557774d3d5bf3c0f2d))
- **Deps:** Aws-sdk-route53 to 1.54.0 - ([7b365b1](https://github.com/beyondessential/bestool/commit/7b365b1d6208ab1f64847f557774d3d5bf3c0f2d))
- **Deps:** Aws-sdk-sts to 1.51.0 - ([7b365b1](https://github.com/beyondessential/bestool/commit/7b365b1d6208ab1f64847f557774d3d5bf3c0f2d))
- **Deps:** Clap to 4.5.23 - ([7b365b1](https://github.com/beyondessential/bestool/commit/7b365b1d6208ab1f64847f557774d3d5bf3c0f2d))
- **Deps:** Detect-targets to 0.1.32 - ([7b365b1](https://github.com/beyondessential/bestool/commit/7b365b1d6208ab1f64847f557774d3d5bf3c0f2d))
- **Deps:** Sysinfo to 0.33.0 - ([7b365b1](https://github.com/beyondessential/bestool/commit/7b365b1d6208ab1f64847f557774d3d5bf3c0f2d))
- **Deps:** Thiserror to 2.0.6 - ([7b365b1](https://github.com/beyondessential/bestool/commit/7b365b1d6208ab1f64847f557774d3d5bf3c0f2d))
- **Deps:** Tokio to 1.42.0 - ([7b365b1](https://github.com/beyondessential/bestool/commit/7b365b1d6208ab1f64847f557774d3d5bf3c0f2d))
- **Deps:** Enable TLS for reqwest - ([39073ef](https://github.com/beyondessential/bestool/commit/39073ef094d285e27329041e6da98b6c1dd0b8f0))
- **Feature:** KAM-296: Backup Configs (#166) - ([fcf94bb](https://github.com/beyondessential/bestool/commit/fcf94bbe9b30c6a85e766e4170f0acf6797bd8c7))

---
## [0.21.4](https://github.com/beyondessential/bestool/compare/v0.21.3..v0.21.4) - 2024-12-05



### Tamanu

- **Bugfix:** Windows compilation - ([12fdd8b](https://github.com/beyondessential/bestool/commit/12fdd8b3a651d4f42c06192ba399be05166c3631))

---
## [0.21.3](https://github.com/beyondessential/bestool/compare/v0.21.2..v0.21.3) - 2024-12-05


- **Deps:** Bump the deps group across 1 directory with 8 updates (#164) - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **Deps:** Update blake3 to 1.5.5 - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **Deps:** Update bytes to 1.9.0 - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **Deps:** Update detect-targets to 0.1.31 - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **Deps:** Update fs4 to 0.12.0 - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **Deps:** Update miette to 7.4.0 - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **Deps:** Update rppal to 0.20.0 - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **Deps:** Update tracing to 0.1.41 - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **Deps:** Update tracing-subscriber to 0.3.19 - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **Deps:** Upgrade mailgun-rs to 1.0.0 - ([9352710](https://github.com/beyondessential/bestool/commit/93527106dc71c7b5ad111d54453e23fb46874961))
- **Test:** Add integration tests to bestool - ([3306f79](https://github.com/beyondessential/bestool/commit/3306f79f552ba7e598dbadc481a133b0169157fa))
- **Tweak:** Improve postgresql binary detection on Linux and Windows - ([3306f79](https://github.com/beyondessential/bestool/commit/3306f79f552ba7e598dbadc481a133b0169157fa))

### Tamanu

- **Bugfix:** Assume facility if we can't detect server type - ([78f605e](https://github.com/beyondessential/bestool/commit/78f605ef92b8edb8962f49ce6c850c591edd5e14))

---
## [0.21.2](https://github.com/beyondessential/bestool/compare/v0.21.1..v0.21.2) - 2024-11-28



### Alerts

- **Documentation:** Fix external targets docs missing targets: line - ([7a80314](https://github.com/beyondessential/bestool/commit/7a803149bc811735c917a3934fb8acf1c3c0384b))

---
## [0.21.1](https://github.com/beyondessential/bestool/compare/v0.21.0..v0.21.1) - 2024-11-28



### Alerts

- **Bugfix:** Warn with specifics when _targets.yml has errors - ([54da4bb](https://github.com/beyondessential/bestool/commit/54da4bb974c3294b045b5db731b6986d8c84225a))

---
## [0.21.0](https://github.com/beyondessential/bestool/compare/v0.20.0..v0.21.0) - 2024-11-26


- **Deps:** Update all - ([9388379](https://github.com/beyondessential/bestool/commit/93883796b659d1524db4dde3bd6f6a282397742a))

### Tamanu

- **Bugfix:** Look into the right places for Linux installs' config - ([7b87111](https://github.com/beyondessential/bestool/commit/7b871115a518b12acc1a4ec2d64683ac6e3c00eb))

---
## [0.20.0](https://github.com/beyondessential/bestool/compare/v0.19.0..v0.20.0) - 2024-11-25



### Psql

- **Feature:** Invert read/write default, require -W, --write to enable write mode - ([d947eb6](https://github.com/beyondessential/bestool/commit/d947eb65130e1b004507e33e877069fa48be487b))

---
## [0.19.0](https://github.com/beyondessential/bestool/compare/v0.18.4..v0.19.0) - 2024-11-23



### Alerts

- **Tweak:** Run scripts as files instead of args - ([10619c3](https://github.com/beyondessential/bestool/commit/10619c3885bd0d5aacb2a514d11b19f2e09ad700))

### Psql

- **Feature:** Add read-only mode - ([fee043c](https://github.com/beyondessential/bestool/commit/fee043c29a3d85ed8acf76a59f9c37cd070da72a))

---
## [0.18.4](https://github.com/beyondessential/bestool/compare/v0.18.3..v0.18.4) - 2024-11-20


- **Bugfix:** Just remove the git build info - ([429fefb](https://github.com/beyondessential/bestool/commit/429fefba54ecd99f13058ceb819297cd246e356e))

---
## [0.18.3](https://github.com/beyondessential/bestool/compare/v0.18.2..v0.18.3) - 2024-11-20


- **Bugfix:** Ability to build with cargo install - ([4a2d724](https://github.com/beyondessential/bestool/commit/4a2d72432cd8666caea418c1393e670d9e8fc2d6))
- **Deps:** Update fs4 requirement from 0.10.0 to 0.11.1 in /crates/bestool (#143) - ([88e06bb](https://github.com/beyondessential/bestool/commit/88e06bb73f4b13ff596aacfd7384d963ba0cd9b3))
- **Deps:** Bump serde_json from 1.0.132 to 1.0.133 (#147) - ([1515a26](https://github.com/beyondessential/bestool/commit/1515a26604d341682fbe03571d913c11a456b116))
- **Deps:** Bump clap_complete from 4.5.33 to 4.5.38 (#145) - ([d43e988](https://github.com/beyondessential/bestool/commit/d43e98885fc47165466561c33b0af2afbdde2770))
- **Deps:** Bump serde from 1.0.210 to 1.0.215 (#146) - ([2a4163c](https://github.com/beyondessential/bestool/commit/2a4163cf6c6ef81aee8f9883a6565b8fbfe4eca2))
- **Deps:** Update rppal requirement from 0.18.0 to 0.19.0 in /crates/bestool (#117) - ([a1d20a7](https://github.com/beyondessential/bestool/commit/a1d20a71df4b2e420816d6a53852c42d7f57a82f))
- **Documentation:** Fix paragraph - ([bbb125f](https://github.com/beyondessential/bestool/commit/bbb125feee581935b20201e30cf9859e84920774))

---
## [0.18.2](https://github.com/beyondessential/bestool/compare/v0.18.1..v0.18.2) - 2024-11-20


- **Deps:** Bump binstalk-downloader from 0.13.1 to 0.13.4 (#142) - ([4ecc305](https://github.com/beyondessential/bestool/commit/4ecc30512dc5f1d173ab7ebff9c7ede030c897b3))

### Self-update

- **Feature:** Add ourselves to PATH on windows with -P - ([c5e4651](https://github.com/beyondessential/bestool/commit/c5e4651628215960175a57ff7837810c0c18785e))

---
## [0.18.1](https://github.com/beyondessential/bestool/compare/v0.18.0..v0.18.1) - 2024-11-20


- **Documentation:** Add flags/commands to docsrs output - ([deca19c](https://github.com/beyondessential/bestool/commit/deca19c34d41271a273eb85d728cdab565a6a2d3))
- **Documentation:** Add docs.rs-only annotations for ease of use - ([ee9750b](https://github.com/beyondessential/bestool/commit/ee9750b351c7235fe84c02fc506f0910560215f6))

---
## [0.18.0](https://github.com/beyondessential/bestool/compare/v0.17.0..v0.18.0) - 2024-11-20


- **Deps:** Bump fs4 from 0.9.1 to 0.10.0 (#136) - ([9511665](https://github.com/beyondessential/bestool/commit/951166556d143b61ee8300a96516d2edf23e88c2))
- **Deps:** Bump serde_yml from 0.0.11 to 0.0.12 (#135) - ([b396216](https://github.com/beyondessential/bestool/commit/b396216824640ada3237a982dd125e9b8e1959fb))
- **Documentation:** Remove obsolete link - ([1f5fc7f](https://github.com/beyondessential/bestool/commit/1f5fc7ffa82552f804a20ffba3915734ac8134ee))

### Alerts

- **Bugfix:** Show errors for alerts parsing - ([9667e16](https://github.com/beyondessential/bestool/commit/9667e16b35ab279c3ddfe51af6dcb00b262533b5))
- **Documentation:** Fix send target syntax - ([ffcd430](https://github.com/beyondessential/bestool/commit/ffcd4301b48b645471d49e9e4dc51eee3bbb173f))
- **Documentation:** Add link to tera - ([0dbbf55](https://github.com/beyondessential/bestool/commit/0dbbf5518b25f9c45f157fc2a2590851d50cfeab))
- **Documentation:** Don't imply that enabled:true is required - ([1989acc](https://github.com/beyondessential/bestool/commit/1989acc9d5a5333ebc0a49ed1138a5cf86147ace))
- **Feature:** Add external targets and docs - ([87228f9](https://github.com/beyondessential/bestool/commit/87228f9d43efa722f99536c6752962ce7d19598d))
- **Style:** More debugging - ([e51f1f0](https://github.com/beyondessential/bestool/commit/e51f1f0210cdadb826884a58c14cb9439cfeaf0f))

### Tamanu

- **Feature:** Add postgres backup tool (#137) - ([4f4c549](https://github.com/beyondessential/bestool/commit/4f4c549c3c996911d19a15b348e1562970fb07fd))

---
## [0.17.0](https://github.com/beyondessential/bestool/compare/v0.16.3..v0.17.0) - 2024-10-24



### Alerts

- **Feature:** KAM-273: add shell script runner (#133) - ([30f6585](https://github.com/beyondessential/bestool/commit/30f6585902155c4a821279dab0fe590c7c9863ed))
- **Feature:** KAM-242: add zendesk as send target (#134) - ([90d269d](https://github.com/beyondessential/bestool/commit/90d269d31c3a22d833d9c8c0ce43ee994902e937))

---
## [0.16.3](https://github.com/beyondessential/bestool/compare/v0.16.2..v0.16.3) - 2024-10-20


- **Bugfix:** Don´t require git in docsrs - ([66b5345](https://github.com/beyondessential/bestool/commit/66b5345536e7a69b5795bb00139a1c93d7915194))

---
## [0.16.2](https://github.com/beyondessential/bestool/compare/v0.16.1..v0.16.2) - 2024-10-20


- **Deps:** Update sysinfo requirement from 0.31.0 to 0.32.0 in /crates/bestool (#127) - ([464e12d](https://github.com/beyondessential/bestool/commit/464e12dcf27a0996714e552546c8965c2dc743f1))

---
## [0.16.1](https://github.com/beyondessential/bestool/compare/v0.16.0..v0.16.1) - 2024-08-29



### Greenmask

- **Bugfix:** Use dunce canonicalize instead of unc - ([476844f](https://github.com/beyondessential/bestool/commit/476844f835832ea1cdb789dd4392eae856854a02))

---
## [0.16.0](https://github.com/beyondessential/bestool/compare/v0.15.2..v0.16.0) - 2024-08-22


- **Deps:** Bump regex from 1.10.5 to 1.10.6 (#105) - ([a02b126](https://github.com/beyondessential/bestool/commit/a02b1262545247a20889f54bb92332baa043105e))
- **Deps:** Bump detect-targets from 0.1.17 to 0.1.18 (#104) - ([d37da99](https://github.com/beyondessential/bestool/commit/d37da99fb62c64c3b4b8bebf415e863b3f8e154b))
- **Deps:** Bump merkle_hash from 3.6.1 to 3.7.0 (#103) - ([3c654c3](https://github.com/beyondessential/bestool/commit/3c654c3b59302a87027257f747e331a2456b7f2f))
- **Deps:** Bump aws-sdk-route53 from 1.37.0 to 1.38.0 (#102) - ([6734035](https://github.com/beyondessential/bestool/commit/6734035234e5f8c595c8c685790c9f3ba3a6bd3c))
- **Deps:** Bump serde_json from 1.0.121 to 1.0.122 (#101) - ([3bf26c1](https://github.com/beyondessential/bestool/commit/3bf26c12bffc2af29cb92541920dcfd24855fe1f))
- **Deps:** Bump aws-sdk-route53 from 1.38.0 to 1.39.0 (#107) - ([a7e58eb](https://github.com/beyondessential/bestool/commit/a7e58ebdbf739455ecadc0df34cb191a3b71a159))
- **Deps:** Bump binstalk-downloader from 0.12.0 to 0.13.0 (#108) - ([04a54f2](https://github.com/beyondessential/bestool/commit/04a54f24b1f1db918d4bbcb88d73b8134f30b324))
- **Deps:** Bump aws-config from 1.5.4 to 1.5.5 (#110) - ([2007e13](https://github.com/beyondessential/bestool/commit/2007e138716391c5bf551caa9c74563c7c988408))
- **Deps:** Bump clap from 4.5.13 to 4.5.15 (#109) - ([e9bb664](https://github.com/beyondessential/bestool/commit/e9bb664039761eaa1aa07836533e7e0aaefceb10))
- **Deps:** Bump bytes from 1.7.0 to 1.7.1 (#106) - ([4c1b2f0](https://github.com/beyondessential/bestool/commit/4c1b2f0c1a06bf0d6257942582b32fd0a6843fbc))
- **Refactor:** Fix missing-feature warnings - ([dca1480](https://github.com/beyondessential/bestool/commit/dca14809ac5affabbc262aac43b3bd273cf5fbfe))
- **Refactor:** Remove console-subscriber feature - ([0ff3ff7](https://github.com/beyondessential/bestool/commit/0ff3ff7831d1c294b33cfcc81bcd68d18e2ab860))
- **Refactor:** Deduplicate subcommands! macro - ([9e077c4](https://github.com/beyondessential/bestool/commit/9e077c43c17dcaf97db1d4acb2cae1042cb78d24))
- **Refactor:** Allow mulitple #[meta] blocks in subcommands! - ([9c81a84](https://github.com/beyondessential/bestool/commit/9c81a8400f6c45ba790342f07c2403aa72552df7))

### Alerts

- **Bugfix:** Bug where templates were shared between alerts - ([ed189f2](https://github.com/beyondessential/bestool/commit/ed189f27c845b944212a77bdfde21378e21976e9))
- **Bugfix:** Only provide as many parameters as are used in the query - ([ed189f2](https://github.com/beyondessential/bestool/commit/ed189f27c845b944212a77bdfde21378e21976e9))
- **Bugfix:** Don't stop after first sendtarget in dry-run - ([ed189f2](https://github.com/beyondessential/bestool/commit/ed189f27c845b944212a77bdfde21378e21976e9))
- **Bugfix:** Only provide as many parameters as are used in the query - ([c3cd05c](https://github.com/beyondessential/bestool/commit/c3cd05ce1112b64d02fb640166eb8e99df41c30c))
- **Bugfix:** Don't stop after first sendtarget in dry-run - ([992043e](https://github.com/beyondessential/bestool/commit/992043ed76c0464393458b284715bf0f95e54fc4))
- **Feature:** Allow sending multiple emails per alert - ([ed189f2](https://github.com/beyondessential/bestool/commit/ed189f27c845b944212a77bdfde21378e21976e9))
- **Feature:** Pass interval to query if wanted - ([ed189f2](https://github.com/beyondessential/bestool/commit/ed189f27c845b944212a77bdfde21378e21976e9))
- **Feature:** Support multiple --dir - ([ed189f2](https://github.com/beyondessential/bestool/commit/ed189f27c845b944212a77bdfde21378e21976e9))
- **Feature:** Pass interval to query if wanted - ([c52cda5](https://github.com/beyondessential/bestool/commit/c52cda52f44b1cdfefce330e308e1c18ab175b65))
- **Feature:** Allow sending multiple emails per alert - ([90010f7](https://github.com/beyondessential/bestool/commit/90010f799d8d89cca1482434d050809a4d956edb))
- **Feature:** Support multiple --dir - ([fea912b](https://github.com/beyondessential/bestool/commit/fea912bccc8a1e9125e18dfdfb0e6180a671369a))
- **Refactor:** Log alert after normalisation - ([ed189f2](https://github.com/beyondessential/bestool/commit/ed189f27c845b944212a77bdfde21378e21976e9))
- **Refactor:** Log alert after normalisation - ([4fe8a3f](https://github.com/beyondessential/bestool/commit/4fe8a3f97f3daaa619251fdfabab4d4a00ffbaad))
- **Test:** Parse an alert - ([ed189f2](https://github.com/beyondessential/bestool/commit/ed189f27c845b944212a77bdfde21378e21976e9))
- **Test:** Parse an alert - ([fe92f06](https://github.com/beyondessential/bestool/commit/fe92f0650c48a567b96304dfda00f6eaa2eb643d))

### Greenmask

- **Bugfix:** Default all paths - ([fc30309](https://github.com/beyondessential/bestool/commit/fc303094694e37b9d692a2cf4ab94bbe678ca60c))
- **Bugfix:** Correct storage stanza - ([604d184](https://github.com/beyondessential/bestool/commit/604d18416441c801bc6cd68013017d1646597451))
- **Feature:** Support multiple config directories - ([c02a898](https://github.com/beyondessential/bestool/commit/c02a8980b33159a04510e13f987598a5b9645e02))
- **Feature:** Look into release folder by default too - ([26ab927](https://github.com/beyondessential/bestool/commit/26ab9278786e562a404818137d79d6412a04a52d))
- **Feature:** Create storage dir if missing - ([1ddc568](https://github.com/beyondessential/bestool/commit/1ddc568f63a147a50f000dbb778b2755d954fc16))

### Tamanu

- **Documentation:** Fix docstring for tamanu download - ([4b36867](https://github.com/beyondessential/bestool/commit/4b368678c444d48aa671d1673e927aad881de603))
- **Feature:** Add greenmask-config command - ([4e874de](https://github.com/beyondessential/bestool/commit/4e874de4c1c61ae9c3c0ebd082d0a430bfc07eca))

---
## [0.15.0](https://github.com/beyondessential/bestool/compare/v0.14.3..v0.15.0) - 2024-08-01


- **Deps:** Upgrade bestool deps - ([ea7dacb](https://github.com/beyondessential/bestool/commit/ea7dacb681abbbcd07365d3155a37a6e35d31a11))
- **Refactor:** Remove upload command - ([eeebc93](https://github.com/beyondessential/bestool/commit/eeebc93c44e6aff8a1dd081a9ad5bc07fb3669ec))

### Alerts

- **Bugfix:** Make interval rendering short and sweet and tested - ([5514551](https://github.com/beyondessential/bestool/commit/55145514fc6ca8765caa095c0a7cdcf44df10650))
- **Refactor:** Split function to more easily test it - ([7d48770](https://github.com/beyondessential/bestool/commit/7d48770ef8685e3643e22b6ffab8b2fd498044c4))

### Crypto

- **Refactor:** Remove minisign subcommands - ([4cb17c6](https://github.com/beyondessential/bestool/commit/4cb17c608b00b420818228c0ac143293c1227ecb))

### Iti

- **Refactor:** Pass upper args through - ([8e1837c](https://github.com/beyondessential/bestool/commit/8e1837cfb82c04de915ace644cf1e0399a56ae46))

---
## [0.14.2](https://github.com/beyondessential/bestool/compare/v0.14.1..v0.14.2) - 2024-07-16



### Alerts

- **Bugfix:** Convert more types than string - ([5d7cf48](https://github.com/beyondessential/bestool/commit/5d7cf48e2e9b7f27d42128aeec7e771d9c85ccde))

---
## [0.14.1](https://github.com/beyondessential/bestool/compare/v0.14.0..v0.14.1) - 2024-07-15


- **Deps:** Update - ([3c0fa7d](https://github.com/beyondessential/bestool/commit/3c0fa7db714002c5e68a6d1acb075e1034d9a388))

---
## [0.14.0](https://github.com/beyondessential/bestool/compare/v0.13.0..v0.14.0) - 2024-07-15


- **Deps:** Bump detect-targets from 0.1.15 to 0.1.17 (#67) - ([daa6db9](https://github.com/beyondessential/bestool/commit/daa6db9e17c59226dd13b87a42047ebae52eb22a))
- **Deps:** Bump binstalk-downloader from 0.10.1 to 0.10.3 (#71) - ([7b00e9e](https://github.com/beyondessential/bestool/commit/7b00e9eb17fb188a2bb519fc96a884874a8648c1))
- **Deps:** Bump boxcar from 0.2.4 to 0.2.5 (#69) - ([a9d5598](https://github.com/beyondessential/bestool/commit/a9d5598e7aea10e54ce43662d30ed8c7135510e8))
- **Deps:** Bump serde from 1.0.200 to 1.0.201 (#68) - ([26edc58](https://github.com/beyondessential/bestool/commit/26edc586a7240dea6cbf135ff12a8245f6f5ab73))
- **Deps:** Bump fs4 from 0.8.2 to 0.8.3 (#70) - ([5bde178](https://github.com/beyondessential/bestool/commit/5bde1786ac1979585fe76f8dcf83d093f43118c9))
- **Deps:** Bump serde_json from 1.0.116 to 1.0.117 (#74) - ([66a40f7](https://github.com/beyondessential/bestool/commit/66a40f7876e8ca492bc45bffd556a8a21a605677))
- **Deps:** Bump aws-config from 1.2.0 to 1.3.0 (#73) - ([07f8272](https://github.com/beyondessential/bestool/commit/07f82726d142119aa52a78c44ed29835d53c9053))
- **Deps:** Bump thiserror from 1.0.59 to 1.0.60 (#72) - ([eb068df](https://github.com/beyondessential/bestool/commit/eb068df18a3234fa2e1ce53321280495a2c0f74d))
- **Deps:** Reduce set of mandatory deps - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **Refactor:** Move bestool crate to a workspace (#65) - ([69cc88c](https://github.com/beyondessential/bestool/commit/69cc88c9ea65b4c8ac6af03177c1597a26cb747b))
- **Refactor:** Split out rpi-st7789v2-driver crate - ([69cc88c](https://github.com/beyondessential/bestool/commit/69cc88c9ea65b4c8ac6af03177c1597a26cb747b))

### Aws

- **Tweak:** Opt into 2024 behaviour (stalled stream protection for uploads) - ([53f428a](https://github.com/beyondessential/bestool/commit/53f428aeb57c32538fc219125efa2102904d238b))

### Iti

- **Bugfix:** Properly clear lcd on start and stop - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **Feature:** Add systemd services for lcd display (#75) - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **Feature:** Add temperature to lcd - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **Feature:** Add local time to lcd - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **Feature:** Add network addresses to lcd - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **Feature:** Add wifi network to lcd - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **Feature:** Sparklines for cpu/ram usage - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **Refactor:** Simplify bg/fg colour calculations - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **Refactor:** Remove wifisetup wip command - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **Tweak:** Make time less precise for battery display - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **Tweak:** More responsive battery display - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **Tweak:** Add fully charged message - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))

<!-- generated by git-cliff -->
