# Changelog

All notable changes to this project will be documented in this file.

See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

---
## [0.12.2](https://github.com/beyondessential/bestool/compare/v0.12.1..0.12.2) - 2024-04-12


- **Bugfix:** Run pm2 with cmd - ([76aa4dd](https://github.com/beyondessential/bestool/commit/76aa4ddc7355396c4e40c0ccf160ea85e37724e7))

---
## [0.12.1](https://github.com/beyondessential/bestool/compare/v0.12.0..v0.12.1) - 2024-04-12



### Tamanu

- **Bugfix:** Run pm2 directly - ([aa33ff4](https://github.com/beyondessential/bestool/commit/aa33ff483cbc12be68ff86485078889ca8af4418))

---
## [0.12.0](https://github.com/beyondessential/bestool/compare/v0.11.0..v0.12.0) - 2024-04-12



### Tamanu

- **Feature:** KAM-198: add `upgrade` command (#40) - ([495bca8](https://github.com/beyondessential/bestool/commit/495bca8031634d2ab8410b9944790aed452a8314))

---
## [0.11.0](https://github.com/beyondessential/bestool/compare/v0.10.1..v0.11.0) - 2024-04-12


- **Deps:** Bump clap from 4.5.3 to 4.5.4 (#34) - ([5978537](https://github.com/beyondessential/bestool/commit/5978537ae46f9d216f421fd055e384c131b0e264))
- **Deps:** Bump aws-sdk-route53 from 1.18.0 to 1.19.0 (#37) - ([f4efe59](https://github.com/beyondessential/bestool/commit/f4efe590bd2ab721d92a1c758ac1179506241cd5))
- **Deps:** Bump aws-sdk-s3 from 1.20.0 to 1.21.0 (#35) - ([a9bed49](https://github.com/beyondessential/bestool/commit/a9bed49318df81cadae1ccd950ca0b3a3807767b))
- **Deps:** Bump chrono from 0.4.35 to 0.4.37 (#36) - ([36f42fd](https://github.com/beyondessential/bestool/commit/36f42fd37c9efbca66bd8863cdb58ff437c7c066))
- **Deps:** Bump h2 from 0.3.25 to 0.3.26 (#41) - ([8053f5c](https://github.com/beyondessential/bestool/commit/8053f5cc753ab94742bce663a43a8faedae150ee))

### Ssh

- **Feature:** Add add-key command (#39) - ([761d0f4](https://github.com/beyondessential/bestool/commit/761d0f473e96fa01ed0a622e61c00c6b0cf559d7))

### Tamanu

- **Feature:** KAM-197: add prepare-upgrade (#38) - ([015439d](https://github.com/beyondessential/bestool/commit/015439d29d9326e9cd1f4f3a3aa6a067b488e3d5))

---
## [0.10.0](https://github.com/beyondessential/bestool/compare/v0.9.1..v0.10.0) - 2024-04-04



### Tamanu

- **Feature:** KAM-10: add psql command (#33) - ([1a54017](https://github.com/beyondessential/bestool/commit/1a54017ab4494bdd00c93c79cb320109a6752075))

---
## [0.9.1](https://github.com/beyondessential/bestool/compare/v0.9.0..v0.9.1) - 2024-04-03


- **Tweak:** Use cache-busting URLs for downloads - ([91e3fe0](https://github.com/beyondessential/bestool/commit/91e3fe0644e86e4aed92f11191bca04837dcebca))

---
## [0.9.0](https://github.com/beyondessential/bestool/compare/v0.8.4..v0.9.0) - 2024-04-03


- **Deps:** Bump tokio from 1.36.0 to 1.37.0 (#31) - ([4990d11](https://github.com/beyondessential/bestool/commit/4990d11d6758ea62eda64e9e2720c2bfba7bb07f))
- **Deps:** Bump serde_json from 1.0.114 to 1.0.115 (#30) - ([50886ce](https://github.com/beyondessential/bestool/commit/50886ce3e58c870df7a989a7ec5d441fe9120cb4))
- **Deps:** Bump aws-config from 1.1.8 to 1.1.9 (#29) - ([2993755](https://github.com/beyondessential/bestool/commit/29937555a38a362b37e11a55cfc61d189baa2686))
- **Deps:** Bump regex from 1.10.3 to 1.10.4 (#28) - ([89b5cc2](https://github.com/beyondessential/bestool/commit/89b5cc2c126c9dcdff462a506196e966bdbfa9c4))
- **Deps:** Bump bytes from 1.5.0 to 1.6.0 (#27) - ([de1c979](https://github.com/beyondessential/bestool/commit/de1c9791638ebc6ab0fae57514e4e6d89a3f22aa))

### Wal-g

- **Feature:** Add wal-g download command - ([ea0290e](https://github.com/beyondessential/bestool/commit/ea0290e3cfda4ad67bec3f6ec9a81e985a96e5d3))

---
## [0.8.4](https://github.com/beyondessential/bestool/compare/v0.8.3..v0.8.4) - 2024-03-22



### Self-update

- **Feature:** Print version after upgrading - ([f5b0cd1](https://github.com/beyondessential/bestool/commit/f5b0cd1ed94cca4562c0a46b8e080a5970e45593))

---
## [0.8.3](https://github.com/beyondessential/bestool/compare/v0.8.2..v0.8.3) - 2024-03-22


- **Documentation:** Mention self-update in readme - ([0b5dd83](https://github.com/beyondessential/bestool/commit/0b5dd832e882a465d3ae71ddb06ccd5b590ef501))

### Self-update

- **Bugfix:** Try to self-update better - ([8c5815e](https://github.com/beyondessential/bestool/commit/8c5815e1670e3dff8a03f09c7b3390ff7320ceda))

---
## [0.8.2](https://github.com/beyondessential/bestool/compare/v0.8.1..v0.8.2) - 2024-03-21


- **Feature:** Enable installing with Binstall - ([b1c5cde](https://github.com/beyondessential/bestool/commit/b1c5cdee1d03ca587f0a643ed18497d739ce073c))

---
## [0.8.1](https://github.com/beyondessential/bestool/compare/v0.8.0..v0.8.1) - 2024-03-21


- **Deps:** Explicit tracing-attributes to get aarch64-gnu to build - ([e542265](https://github.com/beyondessential/bestool/commit/e54226576aac6ea7099f61fa7e090aeb27b23279))
- **Feature:** Control log colour usage with --color - ([eaccc33](https://github.com/beyondessential/bestool/commit/eaccc337f3d6ad7cd817c1f8c82c7ee07efef358))
- **Feature:** Enable ansi colours on windows - ([21d9155](https://github.com/beyondessential/bestool/commit/21d9155e538c6313c7a68c290b325c4f525a0de1))
- **Repo:** Remove broken aarch64-gnu build - ([0587e2b](https://github.com/beyondessential/bestool/commit/0587e2b1d8032f54e7326f64ebadd97ae54ffbac))

---
## [0.8.0](https://github.com/beyondessential/bestool/compare/v0.7.0..v0.8.0) - 2024-03-15



### Crypto

- **Feature:** Add hash subcommand - ([10f1054](https://github.com/beyondessential/bestool/commit/10f105464c133d8e32571b555925ed9b8ad8e2d6))

---
## [0.7.0](https://github.com/beyondessential/bestool/compare/v0.6.1..v0.7.0) - 2024-03-08


- **Deps:** Upgrade all deps (#21) - ([6996788](https://github.com/beyondessential/bestool/commit/6996788190510cb6dd642ff985d8c8763814d038))
- **Documentation:** Add contributing.md and code of conduct - ([88725b0](https://github.com/beyondessential/bestool/commit/88725b0ecf10a2c64d0fdc351cdbf050296dc9c6))
- **Repo:** Open source with GPLv3! - ([548cade](https://github.com/beyondessential/bestool/commit/548cade46b286bbba4ef3370b7427ba29dc2199e))
- **Repo:** Add `tweak` conventional prefix - ([88725b0](https://github.com/beyondessential/bestool/commit/88725b0ecf10a2c64d0fdc351cdbf050296dc9c6))
- **Repo:** Add `wip` conventional prefix - ([88725b0](https://github.com/beyondessential/bestool/commit/88725b0ecf10a2c64d0fdc351cdbf050296dc9c6))
- **Repo:** Enable publishing - ([6b83dbe](https://github.com/beyondessential/bestool/commit/6b83dbe5cce925f3fe55d6c2dfd29b6a7ff18231))
- **Repo:** Fix parsing conventional commit types - ([79828db](https://github.com/beyondessential/bestool/commit/79828db5c885419043b7dbe8af64aca57af328a4))
- **Repo:** Normalise change line casing - ([b76ba9d](https://github.com/beyondessential/bestool/commit/b76ba9df2be391b3b8f2da01fb85fdafec873cc1))

### Eink

- **Feature:** Add eink subcommand - ([1750320](https://github.com/beyondessential/bestool/commit/17503203d99430fdd7097f5ab84c8f485a1817e5))
- **Feature:** Disable eink and dyndns by default - ([5200bd4](https://github.com/beyondessential/bestool/commit/5200bd4e36a6818eb663fb5405e12cbf5146b940))
- **WIP:** Text support - ([f2aa3a1](https://github.com/beyondessential/bestool/commit/f2aa3a1c0a87fd02981da526c607db89dd1df85a))
- **WIP:** Really don't understand what's up with this - ([c22a7cc](https://github.com/beyondessential/bestool/commit/c22a7cc05f8c2e2e057ca5226a5ed4217130475c))

### Upload

- **Test:** Add tests and fix handling of bucket/key arguments - ([818b274](https://github.com/beyondessential/bestool/commit/818b274c94583a962e7d3b8150cfd13885e7a9ae))

---
## [0.6.1](https://github.com/beyondessential/bestool/compare/v0.6.0..v0.6.1) - 2024-02-14


- **Feature:** Print info logs by default - ([24a71fc](https://github.com/beyondessential/bestool/commit/24a71fccd33868deb4c89a73cd965f46522d2f76))

### Caddy

- **Bugfix:** Make downloaded caddy executable on unix - ([3b35c25](https://github.com/beyondessential/bestool/commit/3b35c258380416ab8bb55a373f43fb7db4b8f5d0))
- **Feature:** Add configure-tamanu command - ([01f8fed](https://github.com/beyondessential/bestool/commit/01f8fed2d8d873243ecb3498baf76c60f23bc0d2))

---
## [0.6.0](https://github.com/beyondessential/bestool/compare/v0.5.5..v0.6.0) - 2024-02-14


- **Feature:** Add self-update command - ([292f649](https://github.com/beyondessential/bestool/commit/292f6490f818ab32da87de2c2502b48d29cbe4ef))
- **Feature:** Add caddy command - ([281525c](https://github.com/beyondessential/bestool/commit/281525ccc51a67459b944e7ef2cccdc5de1bb3aa))

### Caddy

- **Bugfix:** Create download path folder if non-extant - ([99d4d6b](https://github.com/beyondessential/bestool/commit/99d4d6bbea1c675fd70060de27f4d4236e01c65c))

### Completions

- **Feature:** Make completions command optional - ([e29aaf1](https://github.com/beyondessential/bestool/commit/e29aaf1ca3957cbcb97fb011456fdd43cd3f18dc))

---
## [0.5.1](https://github.com/beyondessential/bestool/compare/v0.5.0..v0.5.1) - 2024-02-12



### Tamanu

- **Bugfix:** Web package doesn't have a platform - ([9f65771](https://github.com/beyondessential/bestool/commit/9f657719139214b7246326c676205cb45bf1fabe))

---
## [0.5.0](https://github.com/beyondessential/bestool/compare/v0.4.3..v0.5.0) - 2024-02-12



### Crypto

- **Bugfix:** Zero the password after handling - ([b59c632](https://github.com/beyondessential/bestool/commit/b59c632050dc7d498945c8072023f4cf938e2a2e))
- **Refactor:** Rename sign command to crypto - ([6aa4835](https://github.com/beyondessential/bestool/commit/6aa48355084db2cb6b507fec60b7fec5a2a1fb86))
- **Refactor:** Rename check subcommand to verify - ([e70f589](https://github.com/beyondessential/bestool/commit/e70f58981a20d085dd0609793edba92a2b2d27e9))

### Tamanu

- **Feature:** Add download subcommand - ([7368b75](https://github.com/beyondessential/bestool/commit/7368b75bcb8db9f5927c6a9751dec3c1057d24a5))

---
## [0.4.3](https://github.com/beyondessential/bestool/compare/v0.4.2..v0.4.3) - 2024-02-10


- **Documentation:** Show how to use bestool in GHA - ([e5fee6b](https://github.com/beyondessential/bestool/commit/e5fee6b516b07ef76fad02cb5b935fcef0853009))

### Sign

- **Bugfix:** Replace {n} placeholder with {num} - ([edd2633](https://github.com/beyondessential/bestool/commit/edd2633f33ea2672c3b2e6e72479b42a609cacd1))
- **Bugfix:** Signatures filenames would use foo..sig if the input didn't have an extension - ([88e5beb](https://github.com/beyondessential/bestool/commit/88e5bebce6b886f81387897ef8430c4e9d4ddc5e))

---
## [0.4.1](https://github.com/beyondessential/bestool/compare/v0.4.0..v0.4.1) - 2024-02-10


- **Repo:** Try harder to avoid that "chore" type - ([acca607](https://github.com/beyondessential/bestool/commit/acca6074d04016d33c94a96aa72d09fb01e8d89f))

---
## [0.4.0](https://github.com/beyondessential/bestool/compare/v0.3.0..v0.4.0) - 2024-02-10


- **Documentation:** Add downloads for the current version - ([0096050](https://github.com/beyondessential/bestool/commit/0096050a07238e83f3b2058fb2f8a06f732976ac))
- **Documentation:** Provide links to latest URLs - ([d1b6d85](https://github.com/beyondessential/bestool/commit/d1b6d85d222073c24a0fcb86b26d50063896b746))
- **Documentation:** Add development guide - ([2a48820](https://github.com/beyondessential/bestool/commit/2a488205c09e3546fcc0e844ab2fd4f3c888394a))
- **Feature:** Support NO_COLOR (https://no-color.org) - ([8d78d83](https://github.com/beyondessential/bestool/commit/8d78d8317e41824575c4f4b1318ca672e55f8de4))

### Deps

- **Deps:** Bump clap_complete_nushell from 4.4.2 to 4.5.0 (#10) - ([2305cb0](https://github.com/beyondessential/bestool/commit/2305cb05f70711a309bdc8719371f6afd68fa3cc))
- **Deps:** Bump aws-sdk-route53 from 1.13.0 to 1.13.1 (#11) - ([4203344](https://github.com/beyondessential/bestool/commit/4203344c04f7b0be56b0bf1382f16009fcfa1a81))
- **Deps:** Bump serde_json from 1.0.111 to 1.0.113 (#12) - ([bcfb3eb](https://github.com/beyondessential/bestool/commit/bcfb3eb1711b3147d53eaf2b3a998b5082507b11))
- **Deps:** Bump clap_complete from 4.4.9 to 4.5.0 (#13) - ([b2822cc](https://github.com/beyondessential/bestool/commit/b2822cc55b4fbf67a7621a7ac6bd00ac28c4c768))

### Sign

- **Feature:** Add sign command - ([1957417](https://github.com/beyondessential/bestool/commit/1957417e90816d9b239d25d49d910234578d4387))
- **Feature:** Add check command - ([b8af9e0](https://github.com/beyondessential/bestool/commit/b8af9e075c2f65905f8894375a786dd662e3d47e))
- **Feature:** Add keygen command - ([e243510](https://github.com/beyondessential/bestool/commit/e243510a52cf443c30b80710873558837f21cf5a))
- **Refactor:** Extract --output file resolving to reuse into check - ([d87620d](https://github.com/beyondessential/bestool/commit/d87620d1f12fc1c9d3cc1f34956939355852cd8c))

### Tamanu

- **Refactor:** Move roots so tamanu deps can be optional - ([87e5b6d](https://github.com/beyondessential/bestool/commit/87e5b6db5086f7b0fa61ff070b2a23546527c247))

---
## [0.3.0](https://github.com/beyondessential/bestool/compare/v0.2.0..v0.3.0) - 2024-02-09


- **Bugfix:** Clap test - ([5b55a96](https://github.com/beyondessential/bestool/commit/5b55a96904836ab23b581e71bca2d5eb1a95ac58))
- **Deps:** Update deps - ([f8fe1d5](https://github.com/beyondessential/bestool/commit/f8fe1d5b7e2b01a94774b1c33e919b4194859cb1))
- **Feature:** Add progress bars - ([ccafe2c](https://github.com/beyondessential/bestool/commit/ccafe2cbc8420d7980e6702ab65895e63e72c79d))
- **Feature:** Make it possible to turn commands off at compile time - ([1bad1dc](https://github.com/beyondessential/bestool/commit/1bad1dcd1453ae9e12edfebdb91a4d638d4ceec3))
- **Repo:** Add editorconfig - ([5920cc6](https://github.com/beyondessential/bestool/commit/5920cc6468f3d383edae80f083969a02cd756317))
- **Repo:** Don't publish this - ([a9a40bc](https://github.com/beyondessential/bestool/commit/a9a40bc79cb0567887327b714b23522ce43b8f5b))
- **Repo:** Ignore tokens - ([63f15bb](https://github.com/beyondessential/bestool/commit/63f15bb1a4d96c15523ea46089458ffc72c5981d))
- **Repo:** Add release.toml for cargo-release - ([4afa0bc](https://github.com/beyondessential/bestool/commit/4afa0bc1b2c7726adfcc9ae8e0ab807aa30a5aa8))
- **WIP:** Sketch how to cleanup - ([faf37a6](https://github.com/beyondessential/bestool/commit/faf37a601f86968e1eb169cf66146314abb338a5))

### Dyndns

- **Bugfix:** Type ambiguity on Windows - ([7f381a1](https://github.com/beyondessential/bestool/commit/7f381a184b15efcbfa124fb4d147693c5a61848b))
- **Feature:** Add dyndns command for iti - ([78db2cd](https://github.com/beyondessential/bestool/commit/78db2cdc4cdab4d5ebca5d7b89471faf04e8f544))

### S3

- **Bugfix:** Add to cleanup list - ([473c458](https://github.com/beyondessential/bestool/commit/473c458f124098896101c2ffc0d4a7dfd38afbed))

### Upload

- **Feature:** Implement multipart uploads - ([cc19467](https://github.com/beyondessential/bestool/commit/cc19467df8b8b1a99c9a481e3e0868fd083f0a98))
- **Feature:** Implement singlepart uploads - ([69927a4](https://github.com/beyondessential/bestool/commit/69927a4878b3b96cbb5091567065723862c129eb))
- **Feature:** Parse humantime durations - ([0beab30](https://github.com/beyondessential/bestool/commit/0beab307e0379571b96a7249faa7589689805aac))
- **Feature:** Encode tokens - ([b962714](https://github.com/beyondessential/bestool/commit/b962714d773cc12a81a62df0f474a3969f43ca5b))
- **Feature:** Cancel with upload-id - ([242f9cc](https://github.com/beyondessential/bestool/commit/242f9cc814a50af9b6f2856a86332d03723d5a5d))
- **Feature:** Implement preauth upload - ([dc5d40f](https://github.com/beyondessential/bestool/commit/dc5d40f0914495e42d936b1b50242252db97e3fc))
- **Feature:** Prepare other upload commands - ([d046330](https://github.com/beyondessential/bestool/commit/d046330a661c0188a72f4dda54fc760d88f7ff29))
- **Feature:** Attempt delegated tokens - ([a17e3b6](https://github.com/beyondessential/bestool/commit/a17e3b6bc4a11c1270ceb444b009f085050ffbbb))
- **Refactor:** Split aws uploads - ([772b5d2](https://github.com/beyondessential/bestool/commit/772b5d2de5617a7ec9868f9da11392fbeff4a7f1))
- **Refactor:** Common aws args - ([8f829b8](https://github.com/beyondessential/bestool/commit/8f829b80285e95aaea7af447e7d20e740693ef4e))

### Wifisetup

- **Feature:** Implement scan - ([c39af79](https://github.com/beyondessential/bestool/commit/c39af79af02ce2f89c15406e35fab6bdac762b8a))
- **Refactor:** Revise interface for nm - ([3bf9628](https://github.com/beyondessential/bestool/commit/3bf96286f8247efa36663fcaa65c81a277123d3f))
- **WIP:** Sketch wifisetup - ([ce277a3](https://github.com/beyondessential/bestool/commit/ce277a3b6bba8941f6a2f6626c44b1d0cc48527d))
- **WIP:** Disable WIP wifisetup command by default - ([d4417e9](https://github.com/beyondessential/bestool/commit/d4417e91c68329b1ed8a227e1db377192967cd13))

<!-- generated by git-cliff -->
