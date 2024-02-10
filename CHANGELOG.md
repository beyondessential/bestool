# Changelog

All notable changes to this project will be documented in this file.

See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

---
## [0.4.1](https://github.com/beyondessential/bestool/compare/v0.4.0..v0.4.1) - 2024-02-10


- **Repo:** try harder to avoid that "chore" type - ([acca607](https://github.com/beyondessential/bestool/commit/acca6074d04016d33c94a96aa72d09fb01e8d89f))

---
## [0.4.0](https://github.com/beyondessential/bestool/compare/v0.3.0..v0.4.0) - 2024-02-10


- **Documentation:** Add downloads for the current version - ([0096050](https://github.com/beyondessential/bestool/commit/0096050a07238e83f3b2058fb2f8a06f732976ac))
- **Documentation:** Provide links to latest URLs - ([d1b6d85](https://github.com/beyondessential/bestool/commit/d1b6d85d222073c24a0fcb86b26d50063896b746))
- **Documentation:** Add development guide - ([2a48820](https://github.com/beyondessential/bestool/commit/2a488205c09e3546fcc0e844ab2fd4f3c888394a))
- **Feature:** support NO_COLOR (https://no-color.org) - ([8d78d83](https://github.com/beyondessential/bestool/commit/8d78d8317e41824575c4f4b1318ca672e55f8de4))

### Deps

- **Deps:** bump clap_complete_nushell from 4.4.2 to 4.5.0 (#10) - ([2305cb0](https://github.com/beyondessential/bestool/commit/2305cb05f70711a309bdc8719371f6afd68fa3cc))
- **Deps:** bump aws-sdk-route53 from 1.13.0 to 1.13.1 (#11) - ([4203344](https://github.com/beyondessential/bestool/commit/4203344c04f7b0be56b0bf1382f16009fcfa1a81))
- **Deps:** bump serde_json from 1.0.111 to 1.0.113 (#12) - ([bcfb3eb](https://github.com/beyondessential/bestool/commit/bcfb3eb1711b3147d53eaf2b3a998b5082507b11))
- **Deps:** bump clap_complete from 4.4.9 to 4.5.0 (#13) - ([b2822cc](https://github.com/beyondessential/bestool/commit/b2822cc55b4fbf67a7621a7ac6bd00ac28c4c768))

### Sign

- **Feature:** Add sign command - ([1957417](https://github.com/beyondessential/bestool/commit/1957417e90816d9b239d25d49d910234578d4387))
- **Feature:** Add check command - ([b8af9e0](https://github.com/beyondessential/bestool/commit/b8af9e075c2f65905f8894375a786dd662e3d47e))
- **Feature:** Add keygen command - ([e243510](https://github.com/beyondessential/bestool/commit/e243510a52cf443c30b80710873558837f21cf5a))
- **Refactor:** Extract --output file resolving to reuse into check - ([d87620d](https://github.com/beyondessential/bestool/commit/d87620d1f12fc1c9d3cc1f34956939355852cd8c))

### Tamanu

- **Refactor:** Move roots so tamanu deps can be optional - ([87e5b6d](https://github.com/beyondessential/bestool/commit/87e5b6db5086f7b0fa61ff070b2a23546527c247))

---
## [0.3.0] - 2024-02-09


- **Bugfix:** clap test - ([5b55a96](https://github.com/beyondessential/bestool/commit/5b55a96904836ab23b581e71bca2d5eb1a95ac58))
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
