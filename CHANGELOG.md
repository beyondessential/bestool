# Changelog

All notable changes to this project will be documented in this file.

See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

---
## [0.28.2](https://github.com/beyondessential/bestool/compare/v0.28.1..0.28.2) - 2025-03-14


- **Deps:** Bump the deps group with 7 updates (#196) - ([36bc8d3](https://github.com/beyondessential/bestool/commit/36bc8d3230c1085a87e764fb50d33fb618dea15b))

### Backups

- **Bugfix:** Don’t nest backup in duplicate folders when splitting - ([4e7e55d](https://github.com/beyondessential/bestool/commit/4e7e55d75dcb9a705795587bc7f0f9c87f4966da))

---
## [0.28.1](https://github.com/beyondessential/bestool/compare/v0.28.0..v0.28.1) - 2025-03-06


- **Repo:** Add walg feature back so builds don’t break - ([8b1df12](https://github.com/beyondessential/bestool/commit/8b1df127b772106abaac0a3a5ed4ed9e5a3103f0))

---
## [0.28.0](https://github.com/beyondessential/bestool/compare/v0.27.0..v0.28.0) - 2025-03-06


- **Repo:** Completely remove dyndns - ([3783f07](https://github.com/beyondessential/bestool/commit/3783f070bbb1ce6e1cc55234f294f8f3c84fe052))
- **Repo:** Remove useless file_chunker - ([d4e13b1](https://github.com/beyondessential/bestool/commit/d4e13b1dffe18dff060c3818da1161e4e7b0246a))

### Psql

- **Feature:** Arbitrary program and args - ([29a0596](https://github.com/beyondessential/bestool/commit/29a0596e5ecad5e3c4f8909896182dee67a72502))
- **Tweak:** Turn autocommit off when -W is given - ([535d2ee](https://github.com/beyondessential/bestool/commit/535d2ee47d3208a321480e5da50f928f4621b467))

---
## [0.27.0](https://github.com/beyondessential/bestool/compare/v0.26.7..v0.27.0) - 2025-03-05


- **Feature:** KAM-341: split and join files (with backup support) (#194) - ([87e712b](https://github.com/beyondessential/bestool/commit/87e712b4354b9aa7ccc7ed09779b406d07d3af92))

---
## [0.26.7](https://github.com/beyondessential/bestool/compare/v0.26.6..v0.26.7) - 2025-03-04


- **Deps:** Bump the deps group across 1 directory with 14 updates (#188) - ([fd09a5d](https://github.com/beyondessential/bestool/commit/fd09a5d5b910945282c4f270b593afcbab6a0877))
- **Deps:** Bump the deps group across 1 directory with 15 updates (#193) - ([4205bd7](https://github.com/beyondessential/bestool/commit/4205bd7419b797ec2849268a9801b363dcd51708))

### Tamanu

- **Bugfix:** Do not require mailgun until needed - ([25f85fd](https://github.com/beyondessential/bestool/commit/25f85fd0de275dafe6266575c6fdd7e9cc7844b8))

---
## [0.26.5](https://github.com/beyondessential/bestool/compare/v0.26.4..v0.26.5) - 2025-02-04


- **Deps:** Bump the deps group across 1 directory with 9 updates (#186) - ([6681997](https://github.com/beyondessential/bestool/commit/6681997b076e5993c7dd8631dc21077c55540931))
- **Repo:** Remove dyndns feature - ([6d92e03](https://github.com/beyondessential/bestool/commit/6d92e039374f03200c67fb66bc1603b96f259890))

### Downloads

- **Bugfix:** Query tailscale dns directly to avoid buggy systems - ([cc2172e](https://github.com/beyondessential/bestool/commit/cc2172ea14079faa83cf558803e75c93c7b9f59a))

---
## [0.26.4](https://github.com/beyondessential/bestool/compare/v0.26.3..v0.26.4) - 2025-02-04



### Downloads

- **Bugfix:** Use full tailscale name for alternative sources - ([44d82f1](https://github.com/beyondessential/bestool/commit/44d82f1dc6aabec92ae40a92682bab962ba38c85))

---
## [0.26.3](https://github.com/beyondessential/bestool/compare/v0.26.2..v0.26.3) - 2025-02-04


- **Bugfix:** Whoops windows things again - ([0e4a23d](https://github.com/beyondessential/bestool/commit/0e4a23d2b864ca848e586ca66df071033349e565))

---
## [0.26.2](https://github.com/beyondessential/bestool/compare/v0.26.1..v0.26.2) - 2025-02-04


- **Bugfix:** Whoops extraneous `async` - ([e47d1d4](https://github.com/beyondessential/bestool/commit/e47d1d4bd0a38e65811b41240157ba8864e4fcd2))

---
## [0.26.1](https://github.com/beyondessential/bestool/compare/v0.26.0..v0.26.1) - 2025-02-04


- **Refactor:** Use lloggs instead of custom logging code - ([7075b4a](https://github.com/beyondessential/bestool/commit/7075b4a23e8b365606725107204d9fd7ac7ae295))
- **Repo:** Temporarily disable publishing to crates.io - ([6b10528](https://github.com/beyondessential/bestool/commit/6b10528910ac506393ed16427a301e410bd64e24))

### Bestool

- **Feature:** Download from tailscale proxies when available - ([58ca165](https://github.com/beyondessential/bestool/commit/58ca16530171673c14b9c22a44f9d7f4260aefb3))

---
## [0.26.0](https://github.com/beyondessential/bestool/compare/v0.25.8..v0.26.0) - 2025-01-29


- **Bugfix:** Fix tests - ([ef4be42](https://github.com/beyondessential/bestool/commit/ef4be42f3e5112c4e5b4ac600bc8ee25d02528e4))
- **Bugfix:** Fix more tests - ([214ed72](https://github.com/beyondessential/bestool/commit/214ed72d3f388b5e7668eaba28bc7647974ac24d))

### Alerts

- **Feature:** Render email html body from markdown - ([58d9d20](https://github.com/beyondessential/bestool/commit/58d9d20622d0873e0fe602c1e191c25c584d76ec))
- **Refactor:** Split into mods - ([4126fbe](https://github.com/beyondessential/bestool/commit/4126fbe59a512fc88e47f65f7ecf2ab82d59e42a))
- **Refactor:** Split alerts into mods - ([bf8c147](https://github.com/beyondessential/bestool/commit/bf8c14791a582b665c2c7ecf377cb21e39c46ccb))
- **Test:** Remove legacy alert definition support - ([335d5c2](https://github.com/beyondessential/bestool/commit/335d5c219542bb88ca7b26ce417274309c9d1969))

---
## [0.25.8](https://github.com/beyondessential/bestool/compare/v0.25.7..v0.25.8) - 2025-01-28



### Alerts

- **Tweak:** Print which folders are searched - ([09d706a](https://github.com/beyondessential/bestool/commit/09d706a873e956a97068cb16db5de346b4d61898))

---
## [0.25.7](https://github.com/beyondessential/bestool/compare/v0.25.6..v0.25.7) - 2025-01-28



### Alerts

- **Tweak:** Cover more default dirs (toolbox container, cwd) - ([ce4e427](https://github.com/beyondessential/bestool/commit/ce4e427b877a64237bf3cfdb1c9ab0ed82ca7073))

---
## [0.25.6](https://github.com/beyondessential/bestool/compare/v0.25.5..v0.25.6) - 2025-01-28


- **Deps:** Make html2md conditional - ([8d2659e](https://github.com/beyondessential/bestool/commit/8d2659ef478b64d6f46501caca06e167bf2df457))

### Alerts

- **Feature:** Default to reading from the right places - ([e772aa8](https://github.com/beyondessential/bestool/commit/e772aa89d389f667970da58699a733fc3fd72286))

---
## [0.25.5](https://github.com/beyondessential/bestool/compare/v0.25.4..v0.25.5) - 2025-01-28



### Alerts

- **Feature:** Render slack alerts to markdown if they’re html - ([f833063](https://github.com/beyondessential/bestool/commit/f83306392d43f6fec147d1a7ce43d3f8e9122c1a))

---
## [0.25.4](https://github.com/beyondessential/bestool/compare/v0.25.3..v0.25.4) - 2025-01-28



### Alerts

- **Bugfix:** Specify which alert timed out - ([b4f7cef](https://github.com/beyondessential/bestool/commit/b4f7cef1632bb6d0393bba8b69edd2b831c0b426))

---
## [0.25.3](https://github.com/beyondessential/bestool/compare/v0.25.2..v0.25.3) - 2025-01-28



### Alerts

- **Bugfix:** Report on timeouts - ([b34c8e5](https://github.com/beyondessential/bestool/commit/b34c8e5902f17cc8fcb79ac43ef163394e428df2))

---
## [0.25.2](https://github.com/beyondessential/bestool/compare/v0.25.1..v0.25.2) - 2025-01-28



### Alerts

- **Feature:** Add --timeout for alerts to avoid blocking indefinitely - ([4935514](https://github.com/beyondessential/bestool/commit/4935514778fcb82be05e512701a0d844c1896466))
- **Performance:** Run alerts in parallel - ([210ec12](https://github.com/beyondessential/bestool/commit/210ec12787124090c254d6feeb980d860f626375))

---
## [0.25.1](https://github.com/beyondessential/bestool/compare/v0.25.0..v0.25.1) - 2025-01-28



### Backups

- **Tweak:** Use filesystem copy if we can - ([9097b12](https://github.com/beyondessential/bestool/commit/9097b12c54fb910b18d7bf746cc69c5f333c8fc3))

---
## [0.25.0](https://github.com/beyondessential/bestool/compare/v0.24.10..v0.25.0) - 2025-01-28


- **Bugfix:** Fix tests - ([f2866c6](https://github.com/beyondessential/bestool/commit/f2866c65f5b63aa1888f727bce2839624a625243))
- **Style:** Remove a warning - ([6657913](https://github.com/beyondessential/bestool/commit/665791300fbd2fcfac67f5089affbc5fc4fe84ba))

### Alerts

- **Feature:** Add slack and multiplexed external targets - ([692b1c8](https://github.com/beyondessential/bestool/commit/692b1c81f3562c7f2f60bb0f70c7f2cb0dc89463))

### Backups

- **Test:** Remove --deterministic - ([6c2ad7a](https://github.com/beyondessential/bestool/commit/6c2ad7a5fd61f3f7a650251243f70e1f87f3644c))
- **Tweak:** Use zero-compression zips - ([6470673](https://github.com/beyondessential/bestool/commit/6470673e4dc62e6d5166f69ad246e32f4f57707f))

---
## [0.24.10](https://github.com/beyondessential/bestool/compare/v0.24.9..v0.24.10) - 2025-01-24



### Backups

- **Feature:** Create dir for --then-copy-to - ([c2205ac](https://github.com/beyondessential/bestool/commit/c2205ac5e0306858450f6827af8c2bc9debda222))
- **Test:** Can't do deterministic zips - ([da4c706](https://github.com/beyondessential/bestool/commit/da4c706cd23eea55b00016268e994945a691dd19))

---
## [0.24.9](https://github.com/beyondessential/bestool/compare/v0.24.8..v0.24.9) - 2025-01-24



### Backup

- **Test:** Update snapshot - ([e4301f9](https://github.com/beyondessential/bestool/commit/e4301f9c5c5dc8bebbaafa3482e3881b9f1ae37c))

### Backups

- **Feature:** Create dest dir and fix log output - ([869a794](https://github.com/beyondessential/bestool/commit/869a7946328c387920bfc91d3498002571562464))
- **Feature:** Use zip for configs instead of tar - ([e242da7](https://github.com/beyondessential/bestool/commit/e242da7aa04ef74117bbd8ab6d8188e634406509))

---
## [0.24.8](https://github.com/beyondessential/bestool/compare/v0.24.7..v0.24.8) - 2025-01-24


- **Deps:** Bump the deps group with 14 updates (#180) - ([38d1d56](https://github.com/beyondessential/bestool/commit/38d1d56c5fe72a5d1a1c3b561316a017b0089dea))
- **Deps:** Upgrade clap to 4.5.26 - ([38d1d56](https://github.com/beyondessential/bestool/commit/38d1d56c5fe72a5d1a1c3b561316a017b0089dea))
- **Deps:** Upgrade jiff to 0.1.22 - ([38d1d56](https://github.com/beyondessential/bestool/commit/38d1d56c5fe72a5d1a1c3b561316a017b0089dea))
- **Deps:** Upgrade tokio to 1.43.0 - ([38d1d56](https://github.com/beyondessential/bestool/commit/38d1d56c5fe72a5d1a1c3b561316a017b0089dea))
- **Deps:** Upgrade aws-sdk-route53 to 1.58.0 - ([38d1d56](https://github.com/beyondessential/bestool/commit/38d1d56c5fe72a5d1a1c3b561316a017b0089dea))
- **Deps:** Upgrade aws-sdk-sts to 1.54.1 - ([38d1d56](https://github.com/beyondessential/bestool/commit/38d1d56c5fe72a5d1a1c3b561316a017b0089dea))
- **Deps:** Upgrade binstalk-downloader to 0.13.8 - ([38d1d56](https://github.com/beyondessential/bestool/commit/38d1d56c5fe72a5d1a1c3b561316a017b0089dea))
- **Deps:** Upgrade bitflags to 2.7.0 - ([38d1d56](https://github.com/beyondessential/bestool/commit/38d1d56c5fe72a5d1a1c3b561316a017b0089dea))
- **Deps:** Upgrade clap_complete to 4.5.42 - ([38d1d56](https://github.com/beyondessential/bestool/commit/38d1d56c5fe72a5d1a1c3b561316a017b0089dea))
- **Deps:** Upgrade detect-targets to 0.1.36 - ([38d1d56](https://github.com/beyondessential/bestool/commit/38d1d56c5fe72a5d1a1c3b561316a017b0089dea))
- **Deps:** Upgrade dirs to 6.0.0 - ([38d1d56](https://github.com/beyondessential/bestool/commit/38d1d56c5fe72a5d1a1c3b561316a017b0089dea))
- **Deps:** Upgrade serde_json to 1.0.135 - ([38d1d56](https://github.com/beyondessential/bestool/commit/38d1d56c5fe72a5d1a1c3b561316a017b0089dea))
- **Deps:** Upgrade thiserror to 2.0.11 - ([38d1d56](https://github.com/beyondessential/bestool/commit/38d1d56c5fe72a5d1a1c3b561316a017b0089dea))
- **Deps:** Upgrade uuid to 1.11.1 - ([38d1d56](https://github.com/beyondessential/bestool/commit/38d1d56c5fe72a5d1a1c3b561316a017b0089dea))
- **Deps:** Upgrade windows to 0.59.0 - ([38d1d56](https://github.com/beyondessential/bestool/commit/38d1d56c5fe72a5d1a1c3b561316a017b0089dea))

### Backups

- **Feature:** Add encryption and --then-copy-to and --keep-days to config backups - ([fd842a1](https://github.com/beyondessential/bestool/commit/fd842a1bfe3edefea010f8c42059bb60681c39f2))

---
## [0.24.7](https://github.com/beyondessential/bestool/compare/v0.24.6..v0.24.7) - 2025-01-09



### Backups

- **Bugfix:** Compute output filename properly - ([608161e](https://github.com/beyondessential/bestool/commit/608161e7757f359561c6373f34193ac499196cbe))

---
## [0.24.6](https://github.com/beyondessential/bestool/compare/v0.24.5..v0.24.6) - 2025-01-09



### Backups

- **Tweak:** Do file copy in Rust to get a progress indication - ([a16d035](https://github.com/beyondessential/bestool/commit/a16d035593b610b8e927551769621d87e0152fbd))

---
## [0.24.5](https://github.com/beyondessential/bestool/compare/v0.24.4..v0.24.5) - 2025-01-09


- **Style:** Don't mix tokio and std io - ([8017b59](https://github.com/beyondessential/bestool/commit/8017b59bd6ebbe301d1ffef6ab9b0dd9ba157a76))

---
## [0.24.0](https://github.com/beyondessential/bestool/compare/v0.23.3..v0.24.0) - 2025-01-06


- **Deps:** Bump the deps group with 10 updates (#176) - ([a7615d9](https://github.com/beyondessential/bestool/commit/a7615d956cd75d68e107027dd4aa79fd47acc5db))
- **Deps:** Update jiff to 0.1.16 - ([a7615d9](https://github.com/beyondessential/bestool/commit/a7615d956cd75d68e107027dd4aa79fd47acc5db))
- **Deps:** Update aws-config to 1.5.12 - ([a7615d9](https://github.com/beyondessential/bestool/commit/a7615d956cd75d68e107027dd4aa79fd47acc5db))
- **Deps:** Update aws-sdk-route53 to 1.56.0 - ([a7615d9](https://github.com/beyondessential/bestool/commit/a7615d956cd75d68e107027dd4aa79fd47acc5db))
- **Deps:** Update aws-sdk-sts to 1.53.0 - ([a7615d9](https://github.com/beyondessential/bestool/commit/a7615d956cd75d68e107027dd4aa79fd47acc5db))
- **Deps:** Update boxcar to 0.2.8 - ([a7615d9](https://github.com/beyondessential/bestool/commit/a7615d956cd75d68e107027dd4aa79fd47acc5db))
- **Deps:** Update detect-targets to 0.1.34 - ([a7615d9](https://github.com/beyondessential/bestool/commit/a7615d956cd75d68e107027dd4aa79fd47acc5db))
- **Deps:** Update glob to 0.3.2 - ([a7615d9](https://github.com/beyondessential/bestool/commit/a7615d956cd75d68e107027dd4aa79fd47acc5db))
- **Deps:** Update reqwest to 0.12.11 - ([a7615d9](https://github.com/beyondessential/bestool/commit/a7615d956cd75d68e107027dd4aa79fd47acc5db))
- **Deps:** Update serde to 1.0.217 - ([a7615d9](https://github.com/beyondessential/bestool/commit/a7615d956cd75d68e107027dd4aa79fd47acc5db))
- **Deps:** Update sysinfo to 0.33.1 - ([a7615d9](https://github.com/beyondessential/bestool/commit/a7615d956cd75d68e107027dd4aa79fd47acc5db))
- **Deps:** Upgrade itertools to 0.14.0 - ([fc74cb7](https://github.com/beyondessential/bestool/commit/fc74cb72e4a014904fa0d5116163ca6803e6c5f7))

### Backups

- **Feature:** Add --keep-days option to cleanup old backups - ([d2a6df5](https://github.com/beyondessential/bestool/commit/d2a6df55959af328f00a569356947f7f099938cd))

---
## [0.23.1](https://github.com/beyondessential/bestool/compare/v0.23.0..v0.23.1) - 2024-12-24


- **Repo:** Temporarily downgrade algae to 0.0.0 for release purposes - ([3baae1c](https://github.com/beyondessential/bestool/commit/3baae1cc2ac63e3c0a5b36ffefe71ce8d3d4a5f7))

### Crypto

- **Refactor:** Use algae-cli in bestool - ([2d0efe4](https://github.com/beyondessential/bestool/commit/2d0efe481c4ead6a0d05965a3a01dbd477bf27c0))

---
## [0.23.0](https://github.com/beyondessential/bestool/compare/v0.22.0..v0.23.0) - 2024-12-23


- **Deps:** Bump the deps group with 3 updates (#173) - ([0952cef](https://github.com/beyondessential/bestool/commit/0952cef8784698c3c4c7b9c44bf395b177ccb92d))
- **Deps:** Update age to 0.11.1 - ([0952cef](https://github.com/beyondessential/bestool/commit/0952cef8784698c3c4c7b9c44bf395b177ccb92d))
- **Deps:** Update serde_json to 1.0.134 - ([0952cef](https://github.com/beyondessential/bestool/commit/0952cef8784698c3c4c7b9c44bf395b177ccb92d))
- **Deps:** Update thiserror to 2.0.9 - ([0952cef](https://github.com/beyondessential/bestool/commit/0952cef8784698c3c4c7b9c44bf395b177ccb92d))

### Backup

- **Documentation:** Fix help for trailing args - ([e0755c1](https://github.com/beyondessential/bestool/commit/e0755c141843da70ce653d6e60c057e5b607988b))
- **Feature:** KAM-297: add ability to encrypt backups (#174) - ([c9fc6ed](https://github.com/beyondessential/bestool/commit/c9fc6ed7a636aae2d3e4d35f7038a1ed5fb22cee))

### Crypto

- **Documentation:** Explain how to use the identity file in keygen - ([c9fc6ed](https://github.com/beyondessential/bestool/commit/c9fc6ed7a636aae2d3e4d35f7038a1ed5fb22cee))
- **Documentation:** Fix description of keygen - ([41968cc](https://github.com/beyondessential/bestool/commit/41968cc9d22a498e37cea521ae8eddb741d9bd55))
- **Feature:** Add protect/reveal commands for passphrase encryption - ([26a593b](https://github.com/beyondessential/bestool/commit/26a593b3993fb7b87e149952aa8ab70a8a12dece))
- **Feature:** Encrypt identity files by default - ([1c90629](https://github.com/beyondessential/bestool/commit/1c906294967b25d2e8f3495b2a3b0a0cfeadd854))
- **Feature:** Support encrypted identity files directly while en/decrypting - ([91f263b](https://github.com/beyondessential/bestool/commit/91f263b41cb89016358dd711a385ba6a4744a4a1))
- **Feature:** Add --rm to encrypt and protect - ([4344884](https://github.com/beyondessential/bestool/commit/43448843bf6f5c038848def58a9b501a72515d33))
- **Feature:** Write identity.pub by default - ([555662d](https://github.com/beyondessential/bestool/commit/555662df5b38609e276935ac607d49177fbf319e))
- **Refactor:** Extract en/decryption and key handling routines - ([c9fc6ed](https://github.com/beyondessential/bestool/commit/c9fc6ed7a636aae2d3e4d35f7038a1ed5fb22cee))

---
## [0.22.0](https://github.com/beyondessential/bestool/compare/v0.21.5..v0.22.0) - 2024-12-20


- **Deps:** Update rppal requirement from 0.20.0 to 0.22.1 in /crates/bestool in the deps group (#171) - ([44ce528](https://github.com/beyondessential/bestool/commit/44ce5289b66c0667230de2eabd904975a4cf694c))
- **Deps:** Bump the deps group across 1 directory with 10 updates (#172) - ([2d246a4](https://github.com/beyondessential/bestool/commit/2d246a44493b5dbfbfdfe798a4a8966f11ecadf4))
- **Deps:** Update aws-config to 1.5.11 - ([2d246a4](https://github.com/beyondessential/bestool/commit/2d246a44493b5dbfbfdfe798a4a8966f11ecadf4))
- **Deps:** Update aws-sdk-route53 to 1.55.0 - ([2d246a4](https://github.com/beyondessential/bestool/commit/2d246a44493b5dbfbfdfe798a4a8966f11ecadf4))
- **Deps:** Update binstalk-downloader to 0.13.6 - ([2d246a4](https://github.com/beyondessential/bestool/commit/2d246a44493b5dbfbfdfe798a4a8966f11ecadf4))
- **Deps:** Update chrono to 0.4.39 - ([2d246a4](https://github.com/beyondessential/bestool/commit/2d246a44493b5dbfbfdfe798a4a8966f11ecadf4))
- **Deps:** Update clap_complete to 4.5.40 - ([2d246a4](https://github.com/beyondessential/bestool/commit/2d246a44493b5dbfbfdfe798a4a8966f11ecadf4))
- **Deps:** Update detect-targets to 0.1.33 - ([2d246a4](https://github.com/beyondessential/bestool/commit/2d246a44493b5dbfbfdfe798a4a8966f11ecadf4))
- **Deps:** Update rppal to 0.22.1 - ([2d246a4](https://github.com/beyondessential/bestool/commit/2d246a44493b5dbfbfdfe798a4a8966f11ecadf4))
- **Deps:** Update serde to 1.0.216 - ([2d246a4](https://github.com/beyondessential/bestool/commit/2d246a44493b5dbfbfdfe798a4a8966f11ecadf4))

### Crypto

- **Feature:** KAM-297: add encrypt, decrypt, and keygen (#169) - ([c2d24b2](https://github.com/beyondessential/bestool/commit/c2d24b29d465bc4dd43cd77d1e521d7449cd263b))

---
## [0.21.5](https://github.com/beyondessential/bestool/compare/v0.21.4..v0.21.5) - 2024-12-18


- **Deps:** Bump the deps group with 7 updates (#167) - ([2058f7f](https://github.com/beyondessential/bestool/commit/2058f7fdace631408e03d36594f3e43d292f93ae))
- **Deps:** Aws-sdk-route53 to 1.54.0 - ([2058f7f](https://github.com/beyondessential/bestool/commit/2058f7fdace631408e03d36594f3e43d292f93ae))
- **Deps:** Aws-sdk-sts to 1.51.0 - ([2058f7f](https://github.com/beyondessential/bestool/commit/2058f7fdace631408e03d36594f3e43d292f93ae))
- **Deps:** Clap to 4.5.23 - ([2058f7f](https://github.com/beyondessential/bestool/commit/2058f7fdace631408e03d36594f3e43d292f93ae))
- **Deps:** Detect-targets to 0.1.32 - ([2058f7f](https://github.com/beyondessential/bestool/commit/2058f7fdace631408e03d36594f3e43d292f93ae))
- **Deps:** Sysinfo to 0.33.0 - ([2058f7f](https://github.com/beyondessential/bestool/commit/2058f7fdace631408e03d36594f3e43d292f93ae))
- **Deps:** Thiserror to 2.0.6 - ([2058f7f](https://github.com/beyondessential/bestool/commit/2058f7fdace631408e03d36594f3e43d292f93ae))
- **Deps:** Tokio to 1.42.0 - ([2058f7f](https://github.com/beyondessential/bestool/commit/2058f7fdace631408e03d36594f3e43d292f93ae))
- **Deps:** Enable TLS for reqwest - ([a910503](https://github.com/beyondessential/bestool/commit/a91050352f73a628afbf1272ce2cca07f15749e2))
- **Feature:** KAM-296: Backup Configs (#166) - ([93ad98d](https://github.com/beyondessential/bestool/commit/93ad98df8b12b35ba36353d6c09bcf370d3fc06b))

---
## [0.21.4](https://github.com/beyondessential/bestool/compare/v0.21.3..v0.21.4) - 2024-12-05



### Tamanu

- **Bugfix:** Windows compilation - ([96919d4](https://github.com/beyondessential/bestool/commit/96919d494567c2ef3282fbe73f5a80b367e07cf2))

---
## [0.21.3](https://github.com/beyondessential/bestool/compare/v0.21.2..v0.21.3) - 2024-12-05


- **Deps:** Bump the deps group across 1 directory with 8 updates (#164) - ([687c5ee](https://github.com/beyondessential/bestool/commit/687c5eeaa44ad93828c609740951dd5daba98f9a))
- **Deps:** Update blake3 to 1.5.5 - ([687c5ee](https://github.com/beyondessential/bestool/commit/687c5eeaa44ad93828c609740951dd5daba98f9a))
- **Deps:** Update bytes to 1.9.0 - ([687c5ee](https://github.com/beyondessential/bestool/commit/687c5eeaa44ad93828c609740951dd5daba98f9a))
- **Deps:** Update detect-targets to 0.1.31 - ([687c5ee](https://github.com/beyondessential/bestool/commit/687c5eeaa44ad93828c609740951dd5daba98f9a))
- **Deps:** Update fs4 to 0.12.0 - ([687c5ee](https://github.com/beyondessential/bestool/commit/687c5eeaa44ad93828c609740951dd5daba98f9a))
- **Deps:** Update miette to 7.4.0 - ([687c5ee](https://github.com/beyondessential/bestool/commit/687c5eeaa44ad93828c609740951dd5daba98f9a))
- **Deps:** Update rppal to 0.20.0 - ([687c5ee](https://github.com/beyondessential/bestool/commit/687c5eeaa44ad93828c609740951dd5daba98f9a))
- **Deps:** Update tracing to 0.1.41 - ([687c5ee](https://github.com/beyondessential/bestool/commit/687c5eeaa44ad93828c609740951dd5daba98f9a))
- **Deps:** Update tracing-subscriber to 0.3.19 - ([687c5ee](https://github.com/beyondessential/bestool/commit/687c5eeaa44ad93828c609740951dd5daba98f9a))
- **Deps:** Upgrade mailgun-rs to 1.0.0 - ([15abb09](https://github.com/beyondessential/bestool/commit/15abb0931ae2fb8d47381c30de84d0846c556042))
- **Test:** Add integration tests to bestool - ([d1ec56f](https://github.com/beyondessential/bestool/commit/d1ec56f0072c4baf6eb61b9ef2c14ac4893b2109))
- **Tweak:** Improve postgresql binary detection on Linux and Windows - ([d1ec56f](https://github.com/beyondessential/bestool/commit/d1ec56f0072c4baf6eb61b9ef2c14ac4893b2109))

### Tamanu

- **Bugfix:** Assume facility if we can't detect server type - ([ed48e55](https://github.com/beyondessential/bestool/commit/ed48e556dfa064347b90d8a1bf8070118c7fa971))

---
## [0.21.2](https://github.com/beyondessential/bestool/compare/v0.21.1..v0.21.2) - 2024-11-28



### Alerts

- **Documentation:** Fix external targets docs missing targets: line - ([469b7b9](https://github.com/beyondessential/bestool/commit/469b7b9fa3ac27c0f390287b504b6edf861218c9))

---
## [0.21.1](https://github.com/beyondessential/bestool/compare/v0.21.0..v0.21.1) - 2024-11-28



### Alerts

- **Bugfix:** Warn with specifics when _targets.yml has errors - ([4c85002](https://github.com/beyondessential/bestool/commit/4c8500217fcf8a31399533edd837b9ecbd93108d))

---
## [0.21.0](https://github.com/beyondessential/bestool/compare/v0.20.0..v0.21.0) - 2024-11-26


- **Deps:** Update all - ([71bd82e](https://github.com/beyondessential/bestool/commit/71bd82ed343b202d221a5edd19f739335e4434a5))

### Tamanu

- **Bugfix:** Look into the right places for Linux installs' config - ([a385c18](https://github.com/beyondessential/bestool/commit/a385c184034b7a13dce8b7d7e205c87f2a2e1f11))

---
## [0.20.0](https://github.com/beyondessential/bestool/compare/v0.19.0..v0.20.0) - 2024-11-25



### Psql

- **Feature:** Invert read/write default, require -W, --write to enable write mode - ([59d425a](https://github.com/beyondessential/bestool/commit/59d425ade1219723feffadffe75f267b6e1b0996))

---
## [0.19.0](https://github.com/beyondessential/bestool/compare/v0.18.4..v0.19.0) - 2024-11-23



### Alerts

- **Tweak:** Run scripts as files instead of args - ([d5f463c](https://github.com/beyondessential/bestool/commit/d5f463cfa72a2df09c5e323a6aef40562b9fb42e))

### Psql

- **Feature:** Add read-only mode - ([7150d93](https://github.com/beyondessential/bestool/commit/7150d93fdcd4439fe966c2231e689ac1281b0e13))

---
## [0.18.4](https://github.com/beyondessential/bestool/compare/v0.18.3..v0.18.4) - 2024-11-20


- **Bugfix:** Just remove the git build info - ([5e25ffe](https://github.com/beyondessential/bestool/commit/5e25ffe2bfd573ffda88c4a940b791620a21ae58))

---
## [0.18.3](https://github.com/beyondessential/bestool/compare/v0.18.2..v0.18.3) - 2024-11-20


- **Bugfix:** Ability to build with cargo install - ([d4b4a21](https://github.com/beyondessential/bestool/commit/d4b4a21e54595f6cd6451d08faf68c4765719c48))
- **Deps:** Update fs4 requirement from 0.10.0 to 0.11.1 in /crates/bestool (#143) - ([525da60](https://github.com/beyondessential/bestool/commit/525da60eb62f40531bd76637ba19370511a0b742))
- **Deps:** Bump serde_json from 1.0.132 to 1.0.133 (#147) - ([0b10aae](https://github.com/beyondessential/bestool/commit/0b10aaec856f00d5827f1f7801c7d0e2937e91eb))
- **Deps:** Bump clap_complete from 4.5.33 to 4.5.38 (#145) - ([7387b4b](https://github.com/beyondessential/bestool/commit/7387b4b0ab9754d04760ba1599d5a232ec62906d))
- **Deps:** Bump serde from 1.0.210 to 1.0.215 (#146) - ([e6aea17](https://github.com/beyondessential/bestool/commit/e6aea176ff4e53296008596030af34d78b7d2479))
- **Deps:** Update rppal requirement from 0.18.0 to 0.19.0 in /crates/bestool (#117) - ([15111c9](https://github.com/beyondessential/bestool/commit/15111c97f4bd4951a8715ad17ddc6cbcb0f87930))
- **Documentation:** Fix paragraph - ([13708c5](https://github.com/beyondessential/bestool/commit/13708c546aa99a4384fb930a5ec9cd3d14944137))

---
## [0.18.2](https://github.com/beyondessential/bestool/compare/v0.18.1..v0.18.2) - 2024-11-20


- **Deps:** Bump binstalk-downloader from 0.13.1 to 0.13.4 (#142) - ([2509e0b](https://github.com/beyondessential/bestool/commit/2509e0bf080dd6aba7944032e8d67f21cb8ef267))

### Self-update

- **Feature:** Add ourselves to PATH on windows with -P - ([02e234a](https://github.com/beyondessential/bestool/commit/02e234aaf7c370eb5598d53f70c1090e8d7aa7bf))

---
## [0.18.1](https://github.com/beyondessential/bestool/compare/v0.18.0..v0.18.1) - 2024-11-20


- **Documentation:** Add flags/commands to docsrs output - ([e290a67](https://github.com/beyondessential/bestool/commit/e290a67bd06ccb20ec9ee6c605ca252a61ee9437))
- **Documentation:** Add docs.rs-only annotations for ease of use - ([014e036](https://github.com/beyondessential/bestool/commit/014e0366e8fb94f9491dba4ba4e52397c195833c))

---
## [0.18.0](https://github.com/beyondessential/bestool/compare/v0.17.0..v0.18.0) - 2024-11-20


- **Deps:** Bump fs4 from 0.9.1 to 0.10.0 (#136) - ([fe8fdac](https://github.com/beyondessential/bestool/commit/fe8fdacc14c2fc13cb60711dbf978fa57bc76d34))
- **Deps:** Bump serde_yml from 0.0.11 to 0.0.12 (#135) - ([1d26c90](https://github.com/beyondessential/bestool/commit/1d26c90203a22b54aeb0e7959050ee255c91d036))
- **Documentation:** Remove obsolete link - ([6170256](https://github.com/beyondessential/bestool/commit/6170256a98e6b0fce60b845fd9d104e234acac64))

### Alerts

- **Bugfix:** Show errors for alerts parsing - ([e6e5202](https://github.com/beyondessential/bestool/commit/e6e5202c43b23ef5b9609db78ed99e8fc6c26df4))
- **Documentation:** Fix send target syntax - ([b0b8044](https://github.com/beyondessential/bestool/commit/b0b804459623dc33e728da0d22ca1299a3c2ed25))
- **Documentation:** Add link to tera - ([0d392d4](https://github.com/beyondessential/bestool/commit/0d392d44810ee9abcc1e2094d58389dac15a77ec))
- **Documentation:** Don't imply that enabled:true is required - ([6c43663](https://github.com/beyondessential/bestool/commit/6c4366376d956f624eba2900722a628e51c6ae22))
- **Feature:** Add external targets and docs - ([31e90d1](https://github.com/beyondessential/bestool/commit/31e90d160d6974d9d4034e058fb1759b411c187e))
- **Style:** More debugging - ([7048ee0](https://github.com/beyondessential/bestool/commit/7048ee056c1a61b312ab00a67505445b35f5259f))

### Tamanu

- **Feature:** Add postgres backup tool (#137) - ([667171f](https://github.com/beyondessential/bestool/commit/667171fdbc01b0933494167a8b2a7734ee15f752))

---
## [0.17.0](https://github.com/beyondessential/bestool/compare/v0.16.3..v0.17.0) - 2024-10-24



### Alerts

- **Feature:** KAM-273: add shell script runner (#133) - ([8b4e582](https://github.com/beyondessential/bestool/commit/8b4e58288c4c1562ec39828b1d6350bbbda8c46f))
- **Feature:** KAM-242: add zendesk as send target (#134) - ([53177c7](https://github.com/beyondessential/bestool/commit/53177c7873db15c263eef0dadd040ecfcccc0164))

---
## [0.16.3](https://github.com/beyondessential/bestool/compare/v0.16.2..v0.16.3) - 2024-10-20


- **Bugfix:** Don´t require git in docsrs - ([970d935](https://github.com/beyondessential/bestool/commit/970d935e0e42314d7bd469ab8f314897c6e44be3))

---
## [0.16.2](https://github.com/beyondessential/bestool/compare/v0.16.1..v0.16.2) - 2024-10-20


- **Deps:** Update sysinfo requirement from 0.31.0 to 0.32.0 in /crates/bestool (#127) - ([2a8b279](https://github.com/beyondessential/bestool/commit/2a8b2792ad6b49ab27fe4fa63287b80544488719))

---
## [0.16.1](https://github.com/beyondessential/bestool/compare/v0.16.0..v0.16.1) - 2024-08-29



### Greenmask

- **Bugfix:** Use dunce canonicalize instead of unc - ([76304a5](https://github.com/beyondessential/bestool/commit/76304a50d55dfb3dae0fa76ef8d24b1d44f526e1))

---
## [0.16.0](https://github.com/beyondessential/bestool/compare/v0.15.2..v0.16.0) - 2024-08-22


- **Deps:** Bump regex from 1.10.5 to 1.10.6 (#105) - ([b26d804](https://github.com/beyondessential/bestool/commit/b26d8044b94502991c8dbb0dc23666a4f4c1c31c))
- **Deps:** Bump detect-targets from 0.1.17 to 0.1.18 (#104) - ([060d3c1](https://github.com/beyondessential/bestool/commit/060d3c1b5f79cd262c585bcbc3ad32be978c95cd))
- **Deps:** Bump merkle_hash from 3.6.1 to 3.7.0 (#103) - ([1b3bcf3](https://github.com/beyondessential/bestool/commit/1b3bcf34d57e41cd38f20e80724b36e15dc4916a))
- **Deps:** Bump aws-sdk-route53 from 1.37.0 to 1.38.0 (#102) - ([f7b7068](https://github.com/beyondessential/bestool/commit/f7b7068188348270b4c1e54da2268715bd9ffbf4))
- **Deps:** Bump serde_json from 1.0.121 to 1.0.122 (#101) - ([5699343](https://github.com/beyondessential/bestool/commit/56993437ea723871f2ea3383a6e784cc2e10ebd5))
- **Deps:** Bump aws-sdk-route53 from 1.38.0 to 1.39.0 (#107) - ([34672c1](https://github.com/beyondessential/bestool/commit/34672c1cc5e5bdae2659a31eebc4347d3292fbd1))
- **Deps:** Bump binstalk-downloader from 0.12.0 to 0.13.0 (#108) - ([472f0c1](https://github.com/beyondessential/bestool/commit/472f0c1979467e56d776b3e006268d4c4b6a49ae))
- **Deps:** Bump aws-config from 1.5.4 to 1.5.5 (#110) - ([0a5dba7](https://github.com/beyondessential/bestool/commit/0a5dba76d8522885e3e66de367232e1a72a64d50))
- **Deps:** Bump clap from 4.5.13 to 4.5.15 (#109) - ([e4e2e58](https://github.com/beyondessential/bestool/commit/e4e2e58516371a105bed1e342bd70cdb57dea35f))
- **Deps:** Bump bytes from 1.7.0 to 1.7.1 (#106) - ([356bf38](https://github.com/beyondessential/bestool/commit/356bf38fbbec785b0e779a8633de1229851ebe51))
- **Refactor:** Fix missing-feature warnings - ([69e3303](https://github.com/beyondessential/bestool/commit/69e33039631650cc7b5bf52adfb746fc9f513ba2))
- **Refactor:** Remove console-subscriber feature - ([0c244a3](https://github.com/beyondessential/bestool/commit/0c244a30974ec2a5818b99901157e28de1e8a431))
- **Refactor:** Deduplicate subcommands! macro - ([8407c9d](https://github.com/beyondessential/bestool/commit/8407c9dd376e16ce69c874ff3e96b61837f58ad9))
- **Refactor:** Allow mulitple #[meta] blocks in subcommands! - ([ecb174e](https://github.com/beyondessential/bestool/commit/ecb174eed966856538cbf7672ad89b15840af6be))

### Alerts

- **Bugfix:** Bug where templates were shared between alerts - ([5cc49f4](https://github.com/beyondessential/bestool/commit/5cc49f44a2cc412e9869e21f95d8003f9dd86b5c))
- **Bugfix:** Only provide as many parameters as are used in the query - ([5cc49f4](https://github.com/beyondessential/bestool/commit/5cc49f44a2cc412e9869e21f95d8003f9dd86b5c))
- **Bugfix:** Don't stop after first sendtarget in dry-run - ([5cc49f4](https://github.com/beyondessential/bestool/commit/5cc49f44a2cc412e9869e21f95d8003f9dd86b5c))
- **Bugfix:** Only provide as many parameters as are used in the query - ([5d8cb4e](https://github.com/beyondessential/bestool/commit/5d8cb4e4b1a5ebadf7f736a818c7c992b16a4003))
- **Bugfix:** Don't stop after first sendtarget in dry-run - ([5a54311](https://github.com/beyondessential/bestool/commit/5a54311c0afe65b874d7c1ff17c7779694c7ffe2))
- **Feature:** Allow sending multiple emails per alert - ([5cc49f4](https://github.com/beyondessential/bestool/commit/5cc49f44a2cc412e9869e21f95d8003f9dd86b5c))
- **Feature:** Pass interval to query if wanted - ([5cc49f4](https://github.com/beyondessential/bestool/commit/5cc49f44a2cc412e9869e21f95d8003f9dd86b5c))
- **Feature:** Support multiple --dir - ([5cc49f4](https://github.com/beyondessential/bestool/commit/5cc49f44a2cc412e9869e21f95d8003f9dd86b5c))
- **Feature:** Pass interval to query if wanted - ([c2bad97](https://github.com/beyondessential/bestool/commit/c2bad978584cb3d54409b9a68df00164d8ab116d))
- **Feature:** Allow sending multiple emails per alert - ([0600114](https://github.com/beyondessential/bestool/commit/0600114fb322cf25d596e21ab4bd680eb1e3f756))
- **Feature:** Support multiple --dir - ([8efd7ca](https://github.com/beyondessential/bestool/commit/8efd7ca908ea23dd902a2c98f616444a59d9e6c6))
- **Refactor:** Log alert after normalisation - ([5cc49f4](https://github.com/beyondessential/bestool/commit/5cc49f44a2cc412e9869e21f95d8003f9dd86b5c))
- **Refactor:** Log alert after normalisation - ([737e376](https://github.com/beyondessential/bestool/commit/737e3764a59f592a42ba0c3d3cf254aa5389cd6f))
- **Test:** Parse an alert - ([5cc49f4](https://github.com/beyondessential/bestool/commit/5cc49f44a2cc412e9869e21f95d8003f9dd86b5c))
- **Test:** Parse an alert - ([9e9c95b](https://github.com/beyondessential/bestool/commit/9e9c95b7f92f6fe7123df84751d47d93bc093857))

### Greenmask

- **Bugfix:** Default all paths - ([427d7e9](https://github.com/beyondessential/bestool/commit/427d7e90a1336b1687f217e37e36a989f0c51683))
- **Bugfix:** Correct storage stanza - ([3c96f8f](https://github.com/beyondessential/bestool/commit/3c96f8f5e9198ee511d4ea2bd917b69db563fc60))
- **Feature:** Support multiple config directories - ([9e20dad](https://github.com/beyondessential/bestool/commit/9e20dad2034386e7d84beb9fa72bc974721aae13))
- **Feature:** Look into release folder by default too - ([de634c8](https://github.com/beyondessential/bestool/commit/de634c832b12b3267b7aee3b6857d273b606a144))
- **Feature:** Create storage dir if missing - ([7b04bdf](https://github.com/beyondessential/bestool/commit/7b04bdfb46e70e6d5d3064a9344a098ea58e8d0b))

### Tamanu

- **Documentation:** Fix docstring for tamanu download - ([89df2e6](https://github.com/beyondessential/bestool/commit/89df2e60927e9ecfeb2b12e25ee4345789b7537c))
- **Feature:** Add greenmask-config command - ([e922f9a](https://github.com/beyondessential/bestool/commit/e922f9a792f387d8d2e4a55af3ed975673d1e88a))

---
## [0.15.0](https://github.com/beyondessential/bestool/compare/v0.14.3..v0.15.0) - 2024-08-01


- **Deps:** Upgrade bestool deps - ([c8e4831](https://github.com/beyondessential/bestool/commit/c8e48313c696fcc64ce9ac5244815fa390040c6f))
- **Refactor:** Remove upload command - ([2286435](https://github.com/beyondessential/bestool/commit/228643520e7fe6d1c84cc17a18f4da6545fc1c9e))

### Alerts

- **Bugfix:** Make interval rendering short and sweet and tested - ([d4f4517](https://github.com/beyondessential/bestool/commit/d4f45172b143f8d67376d10a1dd5cfc3421b220d))
- **Refactor:** Split function to more easily test it - ([968a775](https://github.com/beyondessential/bestool/commit/968a7756fddcefb5245bb844240eba70defe98dc))

### Crypto

- **Refactor:** Remove minisign subcommands - ([107c0f8](https://github.com/beyondessential/bestool/commit/107c0f85bd302ff7c83bbb9aea51d3b7f7bd0e3a))

### Iti

- **Refactor:** Pass upper args through - ([9d5a93f](https://github.com/beyondessential/bestool/commit/9d5a93fb1f8d9ad487f04b6f807f9a08cc66339c))

---
## [0.14.2](https://github.com/beyondessential/bestool/compare/v0.14.1..v0.14.2) - 2024-07-16



### Alerts

- **Bugfix:** Convert more types than string - ([456500c](https://github.com/beyondessential/bestool/commit/456500c77da0d4b0526c73be99a0a1cfbfdf8398))

---
## [0.14.1](https://github.com/beyondessential/bestool/compare/v0.14.0..v0.14.1) - 2024-07-15


- **Deps:** Update - ([ae784df](https://github.com/beyondessential/bestool/commit/ae784df09e4dcfdccc54ca7d13f703068c835b8d))

---
## [0.14.0](https://github.com/beyondessential/bestool/compare/v0.13.0..v0.14.0) - 2024-07-15


- **Deps:** Bump detect-targets from 0.1.15 to 0.1.17 (#67) - ([e2a5793](https://github.com/beyondessential/bestool/commit/e2a5793b25fcbe0e2a110bdb32c4d49e1985e626))
- **Deps:** Bump binstalk-downloader from 0.10.1 to 0.10.3 (#71) - ([be5d807](https://github.com/beyondessential/bestool/commit/be5d80736476442119e1cf28db4cc2c9fd860912))
- **Deps:** Bump boxcar from 0.2.4 to 0.2.5 (#69) - ([bd0ced2](https://github.com/beyondessential/bestool/commit/bd0ced2967fd8e6955023de0e5b12e0c9ea8ea4b))
- **Deps:** Bump serde from 1.0.200 to 1.0.201 (#68) - ([f7e6eff](https://github.com/beyondessential/bestool/commit/f7e6eff581c168fa38b62f021220ab694b98cad0))
- **Deps:** Bump fs4 from 0.8.2 to 0.8.3 (#70) - ([8718ad6](https://github.com/beyondessential/bestool/commit/8718ad64078cf789c256c4ea36e98f11a516fd81))
- **Deps:** Bump serde_json from 1.0.116 to 1.0.117 (#74) - ([da82a47](https://github.com/beyondessential/bestool/commit/da82a4703f048ad27c728810c1275e63610ea6c8))
- **Deps:** Bump aws-config from 1.2.0 to 1.3.0 (#73) - ([ce8b99a](https://github.com/beyondessential/bestool/commit/ce8b99ac9c0793c3bc3bc317f219206f91679979))
- **Deps:** Bump thiserror from 1.0.59 to 1.0.60 (#72) - ([4f84c8f](https://github.com/beyondessential/bestool/commit/4f84c8f0e9cb5e60e351196e4f1477781373f5aa))
- **Deps:** Reduce set of mandatory deps - ([4a4f768](https://github.com/beyondessential/bestool/commit/4a4f768c18ac7f151adc3ee8df61cc355e7a14ea))
- **Refactor:** Move bestool crate to a workspace (#65) - ([39d8a4b](https://github.com/beyondessential/bestool/commit/39d8a4b8921aa1a5a223d34173e065ef060c6f27))
- **Refactor:** Split out rpi-st7789v2-driver crate - ([39d8a4b](https://github.com/beyondessential/bestool/commit/39d8a4b8921aa1a5a223d34173e065ef060c6f27))

### Aws

- **Tweak:** Opt into 2024 behaviour (stalled stream protection for uploads) - ([55853a0](https://github.com/beyondessential/bestool/commit/55853a0cd3288564c25a5665c8b23575e297fdba))

### Iti

- **Bugfix:** Properly clear lcd on start and stop - ([4a4f768](https://github.com/beyondessential/bestool/commit/4a4f768c18ac7f151adc3ee8df61cc355e7a14ea))
- **Feature:** Add systemd services for lcd display (#75) - ([4a4f768](https://github.com/beyondessential/bestool/commit/4a4f768c18ac7f151adc3ee8df61cc355e7a14ea))
- **Feature:** Add temperature to lcd - ([4a4f768](https://github.com/beyondessential/bestool/commit/4a4f768c18ac7f151adc3ee8df61cc355e7a14ea))
- **Feature:** Add local time to lcd - ([4a4f768](https://github.com/beyondessential/bestool/commit/4a4f768c18ac7f151adc3ee8df61cc355e7a14ea))
- **Feature:** Add network addresses to lcd - ([4a4f768](https://github.com/beyondessential/bestool/commit/4a4f768c18ac7f151adc3ee8df61cc355e7a14ea))
- **Feature:** Add wifi network to lcd - ([4a4f768](https://github.com/beyondessential/bestool/commit/4a4f768c18ac7f151adc3ee8df61cc355e7a14ea))
- **Feature:** Sparklines for cpu/ram usage - ([4a4f768](https://github.com/beyondessential/bestool/commit/4a4f768c18ac7f151adc3ee8df61cc355e7a14ea))
- **Refactor:** Simplify bg/fg colour calculations - ([4a4f768](https://github.com/beyondessential/bestool/commit/4a4f768c18ac7f151adc3ee8df61cc355e7a14ea))
- **Refactor:** Remove wifisetup wip command - ([4a4f768](https://github.com/beyondessential/bestool/commit/4a4f768c18ac7f151adc3ee8df61cc355e7a14ea))
- **Tweak:** Make time less precise for battery display - ([4a4f768](https://github.com/beyondessential/bestool/commit/4a4f768c18ac7f151adc3ee8df61cc355e7a14ea))
- **Tweak:** More responsive battery display - ([4a4f768](https://github.com/beyondessential/bestool/commit/4a4f768c18ac7f151adc3ee8df61cc355e7a14ea))
- **Tweak:** Add fully charged message - ([4a4f768](https://github.com/beyondessential/bestool/commit/4a4f768c18ac7f151adc3ee8df61cc355e7a14ea))

<!-- generated by git-cliff -->
