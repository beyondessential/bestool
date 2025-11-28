# Changelog

All notable changes to this project will be documented in this file.

See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

---
## [1.3.1](https://github.com/beyondessential/bestool/compare/v1.3.0..1.3.1) - 2025-11-28




- **fix(psql):** Try for the manifest in the base version if we can't find the - ([a6c463f](https://github.com/beyondessential/bestool/commit/a6c463f02064ccf249c1b30bd2f25aadbfb1eaf6))
---
## [1.3.0](https://github.com/beyondessential/bestool/compare/v1.2.7..v1.3.0) - 2025-11-28


- **doc:** Update usage - ([5717e51](https://github.com/beyondessential/bestool/commit/5717e51010ecaeb4a0c586af40c80aab826c22f9))


- **feat(psql):** Implement redaction - ([5c02d89](https://github.com/beyondessential/bestool/commit/5c02d898de20f5e6eb49aa3ae9956929c70338f4))
- **feat(psql):** Fetch redactions from dbt manifest - ([2aa48d9](https://github.com/beyondessential/bestool/commit/2aa48d9d07a7053865cf74ab0d6f14e0fb9b3428))
- **refactor(psql):** Make Config proof against future changes - ([a8bf4c7](https://github.com/beyondessential/bestool/commit/a8bf4c7e7dc9d09c6fac660366eb37d67758eec1))
---
## [1.2.7](https://github.com/beyondessential/bestool/compare/v1.2.6..v1.2.7) - 2025-11-27


- **refactor:** Eliminate unsafe code - ([7d181e4](https://github.com/beyondessential/bestool/commit/7d181e44c24237de5cf2d2a3a164aa66dbdb4e47))
- **refactor:** Apply unsafe lint workspace-wide - ([0ca615f](https://github.com/beyondessential/bestool/commit/0ca615fd50daffa93ff874b81ab108ed183d24a5))


- **tweak(backups):** Include fhir materialised resources in the lean backup (#246) - ([7c5ee14](https://github.com/beyondessential/bestool/commit/7c5ee14413eeb411d43af3443511ba824b09f279))
---
## [1.2.6](https://github.com/beyondessential/bestool/compare/v1.2.5..v1.2.6) - 2025-11-26


- **feat:** Produce a deb - ([6e07df2](https://github.com/beyondessential/bestool/commit/6e07df2bf2a3a550ca01257fdee669890aa8c405))

---
## [1.2.4](https://github.com/beyondessential/bestool/compare/v1.2.3..v1.2.4) - 2025-11-24




- **fix(psql):** Argument parsing and overriding - ([aeaa712](https://github.com/beyondessential/bestool/commit/aeaa71287e5a42c7821b88d9ed681672a62393e0))
- **refactor(psql):** Provisionally add facet to our types - ([4f8cc4d](https://github.com/beyondessential/bestool/commit/4f8cc4df6161cf1155efa4b92eb6df3b05a2094b))
---
## [1.2.3](https://github.com/beyondessential/bestool/compare/v1.2.2..v1.2.3) - 2025-11-24


- **feat:** Add _docs hidden command to generate USAGE.md files - ([f08d8ee](https://github.com/beyondessential/bestool/commit/f08d8eedfab94eacab0aea93dfd0f9c1684445fc))
- **tweak:** Support _docs on standalone psql too - ([63cb023](https://github.com/beyondessential/bestool/commit/63cb0236985ca51f5b92a1baa43a778eafe4d323))


- **fix(psql):** Downgrade to notls if sslmode=prefer (default) and tls setup - ([41c0522](https://github.com/beyondessential/bestool/commit/41c0522cf2db60c10d0397362932641558badd5f))
---
## [1.2.0](https://github.com/beyondessential/bestool/compare/v1.1.16..v1.2.0) - 2025-11-20




- **feat(alertd):** Support globs for dirs - ([fbe8fee](https://github.com/beyondessential/bestool/commit/fbe8feefcf9fbc6095341118c29c1b752d1e2af8))
- **feat(alertd):** Add --reload so we can do manual reloads on windows - ([e743c8d](https://github.com/beyondessential/bestool/commit/e743c8def53158f3741b6f10e1214a90e7c111e8))
- **feat(alertd):** Be loud but graceful if we can't bind the server - ([79346a5](https://github.com/beyondessential/bestool/commit/79346a502e3f878e57f34cfa43e6e260c5546048))
- **feat(alertd):** Add loaded-alerts command and endpoint - ([1532392](https://github.com/beyondessential/bestool/commit/15323929ded07d944957a8b7cc8a47c828dafb03))
- **feat(alertd):** Add pause-alert command and endpoint - ([ab0275f](https://github.com/beyondessential/bestool/commit/ab0275f70515f93820268ca52012ba5f08bfd57d))
- **feat(alertd):** Add GET /alerts?detail=true to see the internal state of - ([1217094](https://github.com/beyondessential/bestool/commit/1217094c95d898992a42de457b6034f4d569a91a))
- **refactor(alertd):** Move implementations of commands into the lib - ([3336a59](https://github.com/beyondessential/bestool/commit/3336a590b1751cdcba446ab7d41717b750550039))

- **refactor(bestool):** Update tamanu alertd to use run/reload subcommands - ([8371196](https://github.com/beyondessential/bestool/commit/8371196ce5f031340b64d265a99ab751366472ac))

- **refactor(postgres):** Add postgres_to_json_value and deduplicate from alertd - ([e09c675](https://github.com/beyondessential/bestool/commit/e09c6750419aee36ab413846814a78afb15d8161))
---
## [1.1.16](https://github.com/beyondessential/bestool/compare/v1.1.15..v1.1.16) - 2025-11-20




- **feat(self-update):** Check that we can write the exe before attempting - ([631c9e4](https://github.com/beyondessential/bestool/commit/631c9e43648af17e535509cc70d52bcbeeda0c7f))

- **fix(tamanu):** Encode % in userinfo for db urls - ([ca920aa](https://github.com/beyondessential/bestool/commit/ca920aaec412a0d4689914c2069ba1c84ba88175))
---
## [1.1.15](https://github.com/beyondessential/bestool/compare/v1.1.14..v1.1.15) - 2025-11-18




- **fix(self-update):** Delete temp file before downloading update - ([08b2102](https://github.com/beyondessential/bestool/commit/08b2102559562d5a3d911f67c06b0fa3042effe6))
---
## [1.1.13](https://github.com/beyondessential/bestool/compare/v1.1.12..v1.1.13) - 2025-11-17


- **deps:** Switch reqwest to rustls - ([0893484](https://github.com/beyondessential/bestool/commit/089348402854911ed664a293749c62dead35d34b))
- **deps:** Bump the deps group across 1 directory with 3 updates (#240) - ([7eb3b60](https://github.com/beyondessential/bestool/commit/7eb3b6034915d92bded1ec2b9fbfb5942f2b84bd))


- **feat(psql):** Support unix sockets - ([ef3ca49](https://github.com/beyondessential/bestool/commit/ef3ca49022f9b3939fea141483660ffc62924742))
- **fix(psql):** Special-case /var/sock-like database host - ([b355db6](https://github.com/beyondessential/bestool/commit/b355db664ebf96ba97a1ec2c8dc44c7465cedbab))
---
## [1.1.12](https://github.com/beyondessential/bestool/compare/v1.1.11..v1.1.12) - 2025-11-10




- **fix(psql):** Fix the non-encoded url issue - ([9c2ec21](https://github.com/beyondessential/bestool/commit/9c2ec216781c24ce9665bffaf4b26042a01e616e))
---
## [1.1.11](https://github.com/beyondessential/bestool/compare/v1.1.10..v1.1.11) - 2025-11-04




- **tweak(tamanu/psql):** Log the url for debugging - ([8d231ea](https://github.com/beyondessential/bestool/commit/8d231eab89b9e75b0647b47887f02837059e2f9a))
---
## [1.1.8](https://github.com/beyondessential/bestool/compare/v1.1.7..v1.1.8) - 2025-11-02




- **feat(psql):** Add audit export command - ([453bea3](https://github.com/beyondessential/bestool/commit/453bea3893c9ebca14db652d5cccf395b1956f62))
- **tweak(psql):** --audit-path specifies directory, not file - ([bdb9d1e](https://github.com/beyondessential/bestool/commit/bdb9d1e6a2d1c9444b52574087774fbb26a9e142))
---
## [1.1.2](https://github.com/beyondessential/bestool/compare/v1.1.1..v1.1.2) - 2025-11-01


- **deps:** Uzers is only for unix - ([8281df1](https://github.com/beyondessential/bestool/commit/8281df1d1d11569d9c7bff38aaa47efa699a2513))


- **fix(ssh):** Windows deps again - ([2b36eaa](https://github.com/beyondessential/bestool/commit/2b36eaa247be87e04032b01e0a7cbe14fb3a7c74))
---
## [1.1.1](https://github.com/beyondessential/bestool/compare/v1.1.0..v1.1.1) - 2025-11-01


- **deps:** Replace unsafe serde-yml with unmaintained-but-safe serde-yaml - ([b76d8ec](https://github.com/beyondessential/bestool/commit/b76d8ec8480288f27365a02d16ba6ef869d17762))
- **deps:** Replace unsound users with maintained uzers - ([d76d720](https://github.com/beyondessential/bestool/commit/d76d720df28a94ebea5a25d972bac6f054f36791))

---
## [1.1.0](https://github.com/beyondessential/bestool/compare/v1.0.13..v1.1.0) - 2025-11-01




- **doc(psql):** Write full readme - ([7bb2f3d](https://github.com/beyondessential/bestool/commit/7bb2f3defeb57e8da1eac6e5a16c76065a513fee))
- **feat(psql):** Integrate new psql2 into bestool - ([fee167d](https://github.com/beyondessential/bestool/commit/fee167d7bab5fd93920f6924cdf07a7a5af77ff6))
---
## [1.0.13](https://github.com/beyondessential/bestool/compare/v1.0.12..v1.0.13) - 2025-10-23




- **fix(psql):** Enable OTS prompt on bestool - ([e54a589](https://github.com/beyondessential/bestool/commit/e54a5892a21653cd57e143d8d8cb9f4a5676191d))
---
## [1.0.12](https://github.com/beyondessential/bestool/compare/v1.0.11..v1.0.12) - 2025-10-21




- **fix(update):** Use the right client and construct the url correctly - ([659f0f0](https://github.com/beyondessential/bestool/commit/659f0f06d3480219df408907a4461dad9535aabe))
---
## [1.0.11](https://github.com/beyondessential/bestool/compare/v1.0.10..v1.0.11) - 2025-10-21


- **feat:** Add update self-check - ([223dc66](https://github.com/beyondessential/bestool/commit/223dc66e49279b7028ef070389e03737fbab7647))

---
## [1.0.10](https://github.com/beyondessential/bestool/compare/v1.0.9..v1.0.10) - 2025-10-21


- **feat:** Feat(psql) add --filter to audit tool - ([918d575](https://github.com/beyondessential/bestool/commit/918d575022d5804a043b2f6e0f919de7352569f5))
- **style:** Fix ordering of deps - ([74eea18](https://github.com/beyondessential/bestool/commit/74eea18ff0d10d9c48269899845226a3c6c85ff3))
- **style:** Clippy - ([4524550](https://github.com/beyondessential/bestool/commit/4524550b7f0f15884a816e817bbd5fa86026e15e))


- **feat(psql):** Add --json mode to audit tool - ([c2d22c6](https://github.com/beyondessential/bestool/commit/c2d22c6103c5256848174a57b85f62abab201a81))
- **refactor(psql):** Share code for the export function - ([3888f22](https://github.com/beyondessential/bestool/commit/3888f22eb020964d7f8387acdc0241dc223f553a))
- **refactor(psql):** Simplify audit tool - ([cd64be9](https://github.com/beyondessential/bestool/commit/cd64be94815e23f4e4612647eca60d46eacc5bd6))
---
## [1.0.9](https://github.com/beyondessential/bestool/compare/v1.0.8..v1.0.9) - 2025-10-21




- **refactor(psql):** Clean up legacy field name - ([72f125a](https://github.com/beyondessential/bestool/commit/72f125a4eb9e0c5ea263660769b3185f94670290))
---
## [1.0.4](https://github.com/beyondessential/bestool/compare/v1.0.3..v1.0.4) - 2025-10-21




- **fix(psql):** Disable schema autocompletion by default - ([85d1452](https://github.com/beyondessential/bestool/commit/85d1452093cc3d6f2c9193cb739389574f0723c0))
---
## [1.0.2](https://github.com/beyondessential/bestool/compare/v1.0.1..v1.0.2) - 2025-10-21




- **fix(psql):** Find psql program when not in PATH - ([e1004bb](https://github.com/beyondessential/bestool/commit/e1004bb75f55c3316eb106331b756b7659db1fdd))
---
## [1.0.0](https://github.com/beyondessential/bestool/compare/v0.30.3..v1.0.0) - 2025-10-20


- **deps:** Bump detect-targets from 0.1.15 to 0.1.17 (#67) - ([daa6db9](https://github.com/beyondessential/bestool/commit/daa6db9e17c59226dd13b87a42047ebae52eb22a))
- **deps:** Bump binstalk-downloader from 0.10.1 to 0.10.3 (#71) - ([7b00e9e](https://github.com/beyondessential/bestool/commit/7b00e9eb17fb188a2bb519fc96a884874a8648c1))
- **deps:** Bump boxcar from 0.2.4 to 0.2.5 (#69) - ([a9d5598](https://github.com/beyondessential/bestool/commit/a9d5598e7aea10e54ce43662d30ed8c7135510e8))
- **deps:** Bump serde from 1.0.200 to 1.0.201 (#68) - ([26edc58](https://github.com/beyondessential/bestool/commit/26edc586a7240dea6cbf135ff12a8245f6f5ab73))
- **deps:** Bump fs4 from 0.8.2 to 0.8.3 (#70) - ([5bde178](https://github.com/beyondessential/bestool/commit/5bde1786ac1979585fe76f8dcf83d093f43118c9))
- **deps:** Bump serde_json from 1.0.116 to 1.0.117 (#74) - ([66a40f7](https://github.com/beyondessential/bestool/commit/66a40f7876e8ca492bc45bffd556a8a21a605677))
- **deps:** Bump aws-config from 1.2.0 to 1.3.0 (#73) - ([07f8272](https://github.com/beyondessential/bestool/commit/07f82726d142119aa52a78c44ed29835d53c9053))
- **deps:** Bump thiserror from 1.0.59 to 1.0.60 (#72) - ([eb068df](https://github.com/beyondessential/bestool/commit/eb068df18a3234fa2e1ce53321280495a2c0f74d))
- **deps:** Reduce set of mandatory deps - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **deps:** Update - ([3c0fa7d](https://github.com/beyondessential/bestool/commit/3c0fa7db714002c5e68a6d1acb075e1034d9a388))
- **deps:** Upgrade bestool deps - ([ea7dacb](https://github.com/beyondessential/bestool/commit/ea7dacb681abbbcd07365d3155a37a6e35d31a11))
- **deps:** Bump regex from 1.10.5 to 1.10.6 (#105) - ([a02b126](https://github.com/beyondessential/bestool/commit/a02b1262545247a20889f54bb92332baa043105e))
- **deps:** Bump detect-targets from 0.1.17 to 0.1.18 (#104) - ([d37da99](https://github.com/beyondessential/bestool/commit/d37da99fb62c64c3b4b8bebf415e863b3f8e154b))
- **deps:** Bump merkle_hash from 3.6.1 to 3.7.0 (#103) - ([3c654c3](https://github.com/beyondessential/bestool/commit/3c654c3b59302a87027257f747e331a2456b7f2f))
- **deps:** Bump aws-sdk-route53 from 1.37.0 to 1.38.0 (#102) - ([6734035](https://github.com/beyondessential/bestool/commit/6734035234e5f8c595c8c685790c9f3ba3a6bd3c))
- **deps:** Bump serde_json from 1.0.121 to 1.0.122 (#101) - ([3bf26c1](https://github.com/beyondessential/bestool/commit/3bf26c12bffc2af29cb92541920dcfd24855fe1f))
- **deps:** Bump aws-sdk-route53 from 1.38.0 to 1.39.0 (#107) - ([a7e58eb](https://github.com/beyondessential/bestool/commit/a7e58ebdbf739455ecadc0df34cb191a3b71a159))
- **deps:** Bump binstalk-downloader from 0.12.0 to 0.13.0 (#108) - ([04a54f2](https://github.com/beyondessential/bestool/commit/04a54f24b1f1db918d4bbcb88d73b8134f30b324))
- **deps:** Bump aws-config from 1.5.4 to 1.5.5 (#110) - ([2007e13](https://github.com/beyondessential/bestool/commit/2007e138716391c5bf551caa9c74563c7c988408))
- **deps:** Bump clap from 4.5.13 to 4.5.15 (#109) - ([e9bb664](https://github.com/beyondessential/bestool/commit/e9bb664039761eaa1aa07836533e7e0aaefceb10))
- **deps:** Bump bytes from 1.7.0 to 1.7.1 (#106) - ([4c1b2f0](https://github.com/beyondessential/bestool/commit/4c1b2f0c1a06bf0d6257942582b32fd0a6843fbc))
- **deps:** Update sysinfo requirement from 0.31.0 to 0.32.0 in /crates/bestool (#127) - ([464e12d](https://github.com/beyondessential/bestool/commit/464e12dcf27a0996714e552546c8965c2dc743f1))
- **deps:** Bump fs4 from 0.9.1 to 0.10.0 (#136) - ([9511665](https://github.com/beyondessential/bestool/commit/951166556d143b61ee8300a96516d2edf23e88c2))
- **deps:** Bump serde_yml from 0.0.11 to 0.0.12 (#135) - ([b396216](https://github.com/beyondessential/bestool/commit/b396216824640ada3237a982dd125e9b8e1959fb))
- **deps:** Bump binstalk-downloader from 0.13.1 to 0.13.4 (#142) - ([4ecc305](https://github.com/beyondessential/bestool/commit/4ecc30512dc5f1d173ab7ebff9c7ede030c897b3))
- **deps:** Update fs4 requirement from 0.10.0 to 0.11.1 in /crates/bestool (#143) - ([88e06bb](https://github.com/beyondessential/bestool/commit/88e06bb73f4b13ff596aacfd7384d963ba0cd9b3))
- **deps:** Bump serde_json from 1.0.132 to 1.0.133 (#147) - ([1515a26](https://github.com/beyondessential/bestool/commit/1515a26604d341682fbe03571d913c11a456b116))
- **deps:** Bump clap_complete from 4.5.33 to 4.5.38 (#145) - ([d43e988](https://github.com/beyondessential/bestool/commit/d43e98885fc47165466561c33b0af2afbdde2770))
- **deps:** Bump serde from 1.0.210 to 1.0.215 (#146) - ([2a4163c](https://github.com/beyondessential/bestool/commit/2a4163cf6c6ef81aee8f9883a6565b8fbfe4eca2))
- **deps:** Update rppal requirement from 0.18.0 to 0.19.0 in /crates/bestool (#117) - ([a1d20a7](https://github.com/beyondessential/bestool/commit/a1d20a71df4b2e420816d6a53852c42d7f57a82f))
- **deps:** Update all - ([9388379](https://github.com/beyondessential/bestool/commit/93883796b659d1524db4dde3bd6f6a282397742a))
- **deps:** Bump the deps group across 1 directory with 8 updates (#164) - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **deps:** Update blake3 to 1.5.5 - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **deps:** Update bytes to 1.9.0 - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **deps:** Update detect-targets to 0.1.31 - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **deps:** Update fs4 to 0.12.0 - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **deps:** Update miette to 7.4.0 - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **deps:** Update rppal to 0.20.0 - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **deps:** Update tracing to 0.1.41 - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **deps:** Update tracing-subscriber to 0.3.19 - ([81d9a33](https://github.com/beyondessential/bestool/commit/81d9a33514c2d6df00a6eb3ea3d67c3883376302))
- **deps:** Upgrade mailgun-rs to 1.0.0 - ([9352710](https://github.com/beyondessential/bestool/commit/93527106dc71c7b5ad111d54453e23fb46874961))
- **deps:** Bump the deps group with 7 updates (#167) - ([7b365b1](https://github.com/beyondessential/bestool/commit/7b365b1d6208ab1f64847f557774d3d5bf3c0f2d))
- **deps:** Aws-sdk-route53 to 1.54.0 - ([7b365b1](https://github.com/beyondessential/bestool/commit/7b365b1d6208ab1f64847f557774d3d5bf3c0f2d))
- **deps:** Aws-sdk-sts to 1.51.0 - ([7b365b1](https://github.com/beyondessential/bestool/commit/7b365b1d6208ab1f64847f557774d3d5bf3c0f2d))
- **deps:** Clap to 4.5.23 - ([7b365b1](https://github.com/beyondessential/bestool/commit/7b365b1d6208ab1f64847f557774d3d5bf3c0f2d))
- **deps:** Detect-targets to 0.1.32 - ([7b365b1](https://github.com/beyondessential/bestool/commit/7b365b1d6208ab1f64847f557774d3d5bf3c0f2d))
- **deps:** Sysinfo to 0.33.0 - ([7b365b1](https://github.com/beyondessential/bestool/commit/7b365b1d6208ab1f64847f557774d3d5bf3c0f2d))
- **deps:** Thiserror to 2.0.6 - ([7b365b1](https://github.com/beyondessential/bestool/commit/7b365b1d6208ab1f64847f557774d3d5bf3c0f2d))
- **deps:** Tokio to 1.42.0 - ([7b365b1](https://github.com/beyondessential/bestool/commit/7b365b1d6208ab1f64847f557774d3d5bf3c0f2d))
- **deps:** Enable TLS for reqwest - ([39073ef](https://github.com/beyondessential/bestool/commit/39073ef094d285e27329041e6da98b6c1dd0b8f0))
- **deps:** Update rppal requirement from 0.20.0 to 0.22.1 in /crates/bestool in the deps group (#171) - ([1c4f2d1](https://github.com/beyondessential/bestool/commit/1c4f2d1a6c77f561e1a78dece5ebb8ed2336ba81))
- **deps:** Bump the deps group across 1 directory with 10 updates (#172) - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))
- **deps:** Update aws-config to 1.5.11 - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))
- **deps:** Update aws-sdk-route53 to 1.55.0 - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))
- **deps:** Update binstalk-downloader to 0.13.6 - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))
- **deps:** Update chrono to 0.4.39 - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))
- **deps:** Update clap_complete to 4.5.40 - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))
- **deps:** Update detect-targets to 0.1.33 - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))
- **deps:** Update rppal to 0.22.1 - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))
- **deps:** Update serde to 1.0.216 - ([c14e0f1](https://github.com/beyondessential/bestool/commit/c14e0f1cc4a5f87165d4ed24e412c9e0ec9ef809))
- **deps:** Bump the deps group with 3 updates (#173) - ([688a4f1](https://github.com/beyondessential/bestool/commit/688a4f1ba679bf78d8a86e98751a97a2a789b842))
- **deps:** Update age to 0.11.1 - ([688a4f1](https://github.com/beyondessential/bestool/commit/688a4f1ba679bf78d8a86e98751a97a2a789b842))
- **deps:** Update serde_json to 1.0.134 - ([688a4f1](https://github.com/beyondessential/bestool/commit/688a4f1ba679bf78d8a86e98751a97a2a789b842))
- **deps:** Update thiserror to 2.0.9 - ([688a4f1](https://github.com/beyondessential/bestool/commit/688a4f1ba679bf78d8a86e98751a97a2a789b842))
- **deps:** Bump the deps group with 10 updates (#176) - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **deps:** Update jiff to 0.1.16 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **deps:** Update aws-config to 1.5.12 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **deps:** Update aws-sdk-route53 to 1.56.0 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **deps:** Update aws-sdk-sts to 1.53.0 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **deps:** Update boxcar to 0.2.8 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **deps:** Update detect-targets to 0.1.34 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **deps:** Update glob to 0.3.2 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **deps:** Update reqwest to 0.12.11 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **deps:** Update serde to 1.0.217 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **deps:** Update sysinfo to 0.33.1 - ([c28db6c](https://github.com/beyondessential/bestool/commit/c28db6c4839bfe7013185790ee839c3045adfd24))
- **deps:** Upgrade itertools to 0.14.0 - ([2c7e4c8](https://github.com/beyondessential/bestool/commit/2c7e4c8d132a72454fd74cb02a1b85ad586c12c4))
- **deps:** Bump the deps group with 14 updates (#180) - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **deps:** Upgrade clap to 4.5.26 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **deps:** Upgrade jiff to 0.1.22 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **deps:** Upgrade tokio to 1.43.0 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **deps:** Upgrade aws-sdk-route53 to 1.58.0 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **deps:** Upgrade aws-sdk-sts to 1.54.1 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **deps:** Upgrade binstalk-downloader to 0.13.8 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **deps:** Upgrade bitflags to 2.7.0 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **deps:** Upgrade clap_complete to 4.5.42 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **deps:** Upgrade detect-targets to 0.1.36 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **deps:** Upgrade dirs to 6.0.0 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **deps:** Upgrade serde_json to 1.0.135 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **deps:** Upgrade thiserror to 2.0.11 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **deps:** Upgrade uuid to 1.11.1 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **deps:** Upgrade windows to 0.59.0 - ([a1efed8](https://github.com/beyondessential/bestool/commit/a1efed8cdfef53e3888cd2357ec5064254cbdb28))
- **deps:** Make html2md conditional - ([a0af4fa](https://github.com/beyondessential/bestool/commit/a0af4fa054a519f1c70e47773516628acb4a379c))
- **deps:** Bump the deps group across 1 directory with 9 updates (#186) - ([706ea9d](https://github.com/beyondessential/bestool/commit/706ea9dc8e6958e40b24e7a5efff18d1f1fd9896))
- **deps:** Bump the deps group across 1 directory with 14 updates (#188) - ([650984b](https://github.com/beyondessential/bestool/commit/650984b23fb7bc6bb12b722489f10588d2747c62))
- **deps:** Bump the deps group across 1 directory with 15 updates (#193) - ([751581d](https://github.com/beyondessential/bestool/commit/751581df704570d3eaa57ef1a64f8da152fb2d79))
- **deps:** Bump the deps group with 7 updates (#196) - ([bbdf6c0](https://github.com/beyondessential/bestool/commit/bbdf6c0cfcaa489684aefca3757b7fcb568eb668))
- **deps:** Bump the deps group with 11 updates (#197) - ([2d6e431](https://github.com/beyondessential/bestool/commit/2d6e431759cf24a492881cc3ce0d62bc1c473105))
- **deps:** Bump the deps group across 1 directory with 32 updates (#217) - ([d8693fc](https://github.com/beyondessential/bestool/commit/d8693fc00b4cb98a269488f0d3c4cf4a89646015))
- **deps:** Bump the deps group with 8 updates (#218) - ([d4003b2](https://github.com/beyondessential/bestool/commit/d4003b2b9fcbeaf73ce6994c1f146c56c8da154b))
- **deps:** Bump the deps group with 6 updates (#220) - ([93e8e9f](https://github.com/beyondessential/bestool/commit/93e8e9f86ff28f0d158f3891d850ab01eba7dc90))
- **deps:** Bump the deps group with 6 updates (#222) - ([17c8340](https://github.com/beyondessential/bestool/commit/17c83404bec0be56929530e6e6457d653e2e74a9))
- **deps:** Upgrade psql deps - ([2575068](https://github.com/beyondessential/bestool/commit/2575068d9da8e8bd585d4fb6b5d2c4eb9079111c))
- **deps:** Fix optional deps - ([d1c79ea](https://github.com/beyondessential/bestool/commit/d1c79ea929bb6140c8a73856a400263f9a9b3213))
- **doc:** Remove obsolete link - ([1f5fc7f](https://github.com/beyondessential/bestool/commit/1f5fc7ffa82552f804a20ffba3915734ac8134ee))
- **doc:** Add flags/commands to docsrs output - ([deca19c](https://github.com/beyondessential/bestool/commit/deca19c34d41271a273eb85d728cdab565a6a2d3))
- **doc:** Add docs.rs-only annotations for ease of use - ([ee9750b](https://github.com/beyondessential/bestool/commit/ee9750b351c7235fe84c02fc506f0910560215f6))
- **doc:** Fix paragraph - ([bbb125f](https://github.com/beyondessential/bestool/commit/bbb125feee581935b20201e30cf9859e84920774))
- **doc:** Document aliases - ([4308848](https://github.com/beyondessential/bestool/commit/430884834f42a078bf200bb49a3d8672c23ccb78))
- **feat:** KAM-296: Backup Configs (#166) - ([fcf94bb](https://github.com/beyondessential/bestool/commit/fcf94bbe9b30c6a85e766e4170f0acf6797bd8c7))
- **feat:** KAM-341: split and join files (with backup support) (#194) - ([ea3e9f9](https://github.com/beyondessential/bestool/commit/ea3e9f9737f1db5460e8666a890e44c212524bc0))
- **fix:** Don´t require git in docsrs - ([66b5345](https://github.com/beyondessential/bestool/commit/66b5345536e7a69b5795bb00139a1c93d7915194))
- **fix:** Ability to build with cargo install - ([4a2d724](https://github.com/beyondessential/bestool/commit/4a2d72432cd8666caea418c1393e670d9e8fc2d6))
- **fix:** Just remove the git build info - ([429fefb](https://github.com/beyondessential/bestool/commit/429fefba54ecd99f13058ceb819297cd246e356e))
- **fix:** Fix tests - ([7a383e5](https://github.com/beyondessential/bestool/commit/7a383e5132d506264d7f06d1332e427034affe7f))
- **fix:** Fix tests - ([e94131c](https://github.com/beyondessential/bestool/commit/e94131cc54096669b7692f68253ebba6b8e1ad49))
- **fix:** Fix more tests - ([65951f8](https://github.com/beyondessential/bestool/commit/65951f8ff0b4435051094dabda4ad67a2347ccfe))
- **fix:** Whoops extraneous `async` - ([96ccc26](https://github.com/beyondessential/bestool/commit/96ccc2683a2da3914044a0af0b44aee6bedbfe6b))
- **fix:** Whoops windows things again - ([c632a00](https://github.com/beyondessential/bestool/commit/c632a00457805a1e54233de75608ecf24ed48eae))
- **fix:** Codepage setting - ([44f0317](https://github.com/beyondessential/bestool/commit/44f03172da24129b82dc5151b7776cd7206ba7ce))
- **fix:** Default-run - ([eead678](https://github.com/beyondessential/bestool/commit/eead6781887e25a493439ab2b3ea8c97f59c67d4))
- **refactor:** Move bestool crate to a workspace (#65) - ([69cc88c](https://github.com/beyondessential/bestool/commit/69cc88c9ea65b4c8ac6af03177c1597a26cb747b))
- **refactor:** Split out rpi-st7789v2-driver crate - ([69cc88c](https://github.com/beyondessential/bestool/commit/69cc88c9ea65b4c8ac6af03177c1597a26cb747b))
- **refactor:** Remove upload command - ([eeebc93](https://github.com/beyondessential/bestool/commit/eeebc93c44e6aff8a1dd081a9ad5bc07fb3669ec))
- **refactor:** Fix missing-feature warnings - ([dca1480](https://github.com/beyondessential/bestool/commit/dca14809ac5affabbc262aac43b3bd273cf5fbfe))
- **refactor:** Remove console-subscriber feature - ([0ff3ff7](https://github.com/beyondessential/bestool/commit/0ff3ff7831d1c294b33cfcc81bcd68d18e2ab860))
- **refactor:** Deduplicate subcommands! macro - ([9e077c4](https://github.com/beyondessential/bestool/commit/9e077c43c17dcaf97db1d4acb2cae1042cb78d24))
- **refactor:** Allow mulitple #[meta] blocks in subcommands! - ([9c81a84](https://github.com/beyondessential/bestool/commit/9c81a8400f6c45ba790342f07c2403aa72552df7))
- **refactor:** Use lloggs instead of custom logging code - ([0297fdc](https://github.com/beyondessential/bestool/commit/0297fdc3bb30584e3cb28effdf3645f8a3b5197a))
- **repo:** Temporarily downgrade algae to 0.0.0 for release purposes - ([9d564c6](https://github.com/beyondessential/bestool/commit/9d564c6670af75f952c86733b908e8fd6ac3266a))
- **repo:** Temporarily disable publishing to crates.io - ([8e8dd29](https://github.com/beyondessential/bestool/commit/8e8dd29c10706be45a4f5712b81be859d22c1f13))
- **repo:** Remove dyndns feature - ([6e33015](https://github.com/beyondessential/bestool/commit/6e33015d33b66bcfcdc5321209442bbd4da78797))
- **repo:** Completely remove dyndns - ([8bacd55](https://github.com/beyondessential/bestool/commit/8bacd55cacd14315599e69a3875543ce3262a7d9))
- **repo:** Remove useless file_chunker - ([16d55f5](https://github.com/beyondessential/bestool/commit/16d55f556b0a171b56f80e4bcfa8eccf07266955))
- **repo:** Add walg feature back so builds don’t break - ([f5908df](https://github.com/beyondessential/bestool/commit/f5908dfb863bbd7327b82dabc1e4e83ad3e934e6))
- **style:** Don't mix tokio and std io - ([bb6e07c](https://github.com/beyondessential/bestool/commit/bb6e07c9ef4a7b68b72592f9f04fad69135227b9))
- **style:** Remove a warning - ([03932be](https://github.com/beyondessential/bestool/commit/03932be80b2dfc80eb7fcdf84923a88a92c5ae23))
- **style:** Fix clippy - ([a5a571f](https://github.com/beyondessential/bestool/commit/a5a571f9d27177941978f934a8f508991918a344))
- **style:** Use proper types and traits - ([30ce373](https://github.com/beyondessential/bestool/commit/30ce3734bf8206a2723f4f1ec82c1004235b8a32))
- **test:** Add integration tests to bestool - ([3306f79](https://github.com/beyondessential/bestool/commit/3306f79f552ba7e598dbadc481a133b0169157fa))
- **tweak:** Improve postgresql binary detection on Linux and Windows - ([3306f79](https://github.com/beyondessential/bestool/commit/3306f79f552ba7e598dbadc481a133b0169157fa))
- **tweak:** Use hand-picked short aliases instead of inferred shorthands - ([1f1312e](https://github.com/beyondessential/bestool/commit/1f1312e9b4afff7135de87de7251a2f0c6643588))


- **doc(alerts):** Fix send target syntax - ([ffcd430](https://github.com/beyondessential/bestool/commit/ffcd4301b48b645471d49e9e4dc51eee3bbb173f))
- **doc(alerts):** Add link to tera - ([0dbbf55](https://github.com/beyondessential/bestool/commit/0dbbf5518b25f9c45f157fc2a2590851d50cfeab))
- **doc(alerts):** Don't imply that enabled:true is required - ([1989acc](https://github.com/beyondessential/bestool/commit/1989acc9d5a5333ebc0a49ed1138a5cf86147ace))
- **doc(alerts):** Fix external targets docs missing targets: line - ([7a80314](https://github.com/beyondessential/bestool/commit/7a803149bc811735c917a3934fb8acf1c3c0384b))
- **feat(alerts):** Allow sending multiple emails per alert - ([ed189f2](https://github.com/beyondessential/bestool/commit/ed189f27c845b944212a77bdfde21378e21976e9))
- **feat(alerts):** Pass interval to query if wanted - ([ed189f2](https://github.com/beyondessential/bestool/commit/ed189f27c845b944212a77bdfde21378e21976e9))
- **feat(alerts):** Support multiple --dir - ([ed189f2](https://github.com/beyondessential/bestool/commit/ed189f27c845b944212a77bdfde21378e21976e9))
- **feat(alerts):** Pass interval to query if wanted - ([c52cda5](https://github.com/beyondessential/bestool/commit/c52cda52f44b1cdfefce330e308e1c18ab175b65))
- **feat(alerts):** Allow sending multiple emails per alert - ([90010f7](https://github.com/beyondessential/bestool/commit/90010f799d8d89cca1482434d050809a4d956edb))
- **feat(alerts):** Support multiple --dir - ([fea912b](https://github.com/beyondessential/bestool/commit/fea912bccc8a1e9125e18dfdfb0e6180a671369a))
- **feat(alerts):** KAM-273: add shell script runner (#133) - ([30f6585](https://github.com/beyondessential/bestool/commit/30f6585902155c4a821279dab0fe590c7c9863ed))
- **feat(alerts):** KAM-242: add zendesk as send target (#134) - ([90d269d](https://github.com/beyondessential/bestool/commit/90d269d31c3a22d833d9c8c0ce43ee994902e937))
- **feat(alerts):** Add external targets and docs - ([87228f9](https://github.com/beyondessential/bestool/commit/87228f9d43efa722f99536c6752962ce7d19598d))
- **feat(alerts):** Add slack and multiplexed external targets - ([5573478](https://github.com/beyondessential/bestool/commit/5573478428760cc3642cf62cfd380aa175e3cbe4))
- **feat(alerts):** Add --timeout for alerts to avoid blocking indefinitely - ([889301c](https://github.com/beyondessential/bestool/commit/889301cbac2fb1ac494c9bbe1db5009696216994))
- **feat(alerts):** Render slack alerts to markdown if they’re html - ([110a90a](https://github.com/beyondessential/bestool/commit/110a90ae88e5fadcfc049a97aa58e333876c854b))
- **feat(alerts):** Default to reading from the right places - ([b4834f7](https://github.com/beyondessential/bestool/commit/b4834f7ce81dbb1132f9c153763d52e4e2724a60))
- **feat(alerts):** Render email html body from markdown - ([d4bb1e8](https://github.com/beyondessential/bestool/commit/d4bb1e8915aae77d3e04caf80d2997b48fac0f29))
- **fix(alerts):** Convert more types than string - ([5d7cf48](https://github.com/beyondessential/bestool/commit/5d7cf48e2e9b7f27d42128aeec7e771d9c85ccde))
- **fix(alerts):** Make interval rendering short and sweet and tested - ([5514551](https://github.com/beyondessential/bestool/commit/55145514fc6ca8765caa095c0a7cdcf44df10650))
- **fix(alerts):** Bug where templates were shared between alerts - ([ed189f2](https://github.com/beyondessential/bestool/commit/ed189f27c845b944212a77bdfde21378e21976e9))
- **fix(alerts):** Only provide as many parameters as are used in the query - ([ed189f2](https://github.com/beyondessential/bestool/commit/ed189f27c845b944212a77bdfde21378e21976e9))
- **fix(alerts):** Don't stop after first sendtarget in dry-run - ([ed189f2](https://github.com/beyondessential/bestool/commit/ed189f27c845b944212a77bdfde21378e21976e9))
- **fix(alerts):** Only provide as many parameters as are used in the query - ([c3cd05c](https://github.com/beyondessential/bestool/commit/c3cd05ce1112b64d02fb640166eb8e99df41c30c))
- **fix(alerts):** Don't stop after first sendtarget in dry-run - ([992043e](https://github.com/beyondessential/bestool/commit/992043ed76c0464393458b284715bf0f95e54fc4))
- **fix(alerts):** Show errors for alerts parsing - ([9667e16](https://github.com/beyondessential/bestool/commit/9667e16b35ab279c3ddfe51af6dcb00b262533b5))
- **fix(alerts):** Warn with specifics when _targets.yml has errors - ([54da4bb](https://github.com/beyondessential/bestool/commit/54da4bb974c3294b045b5db731b6986d8c84225a))
- **fix(alerts):** Report on timeouts - ([97fe962](https://github.com/beyondessential/bestool/commit/97fe9627e814f6af36867916c15d6144f8fb606e))
- **fix(alerts):** Specify which alert timed out - ([c563b1e](https://github.com/beyondessential/bestool/commit/c563b1e5d5632c909e51e50673165c5c5ea0f53b))
- **perf(alerts):** Run alerts in parallel - ([4246c11](https://github.com/beyondessential/bestool/commit/4246c1114ead79edffaef201c8acde605e786fc8))
- **refactor(alerts):** Split function to more easily test it - ([7d48770](https://github.com/beyondessential/bestool/commit/7d48770ef8685e3643e22b6ffab8b2fd498044c4))
- **refactor(alerts):** Log alert after normalisation - ([ed189f2](https://github.com/beyondessential/bestool/commit/ed189f27c845b944212a77bdfde21378e21976e9))
- **refactor(alerts):** Log alert after normalisation - ([4fe8a3f](https://github.com/beyondessential/bestool/commit/4fe8a3f97f3daaa619251fdfabab4d4a00ffbaad))
- **refactor(alerts):** Split into mods - ([af98c55](https://github.com/beyondessential/bestool/commit/af98c555632f9a8370b2de2982cb3944ed58fbcd))
- **refactor(alerts):** Split alerts into mods - ([f7a5407](https://github.com/beyondessential/bestool/commit/f7a54070dd0c563f436022dcbf53eb1a08f7353a))
- **style(alerts):** More debugging - ([e51f1f0](https://github.com/beyondessential/bestool/commit/e51f1f0210cdadb826884a58c14cb9439cfeaf0f))
- **test(alerts):** Parse an alert - ([ed189f2](https://github.com/beyondessential/bestool/commit/ed189f27c845b944212a77bdfde21378e21976e9))
- **test(alerts):** Parse an alert - ([fe92f06](https://github.com/beyondessential/bestool/commit/fe92f0650c48a567b96304dfda00f6eaa2eb643d))
- **test(alerts):** Remove legacy alert definition support - ([155e79b](https://github.com/beyondessential/bestool/commit/155e79bb7c03debe494afc46d80dbe6e1be08e08))
- **tweak(alerts):** Run scripts as files instead of args - ([10619c3](https://github.com/beyondessential/bestool/commit/10619c3885bd0d5aacb2a514d11b19f2e09ad700))
- **tweak(alerts):** Cover more default dirs (toolbox container, cwd) - ([7a2a582](https://github.com/beyondessential/bestool/commit/7a2a5826d646599f4b31168bb412ea1a4dfec8d0))
- **tweak(alerts):** Print which folders are searched - ([2916298](https://github.com/beyondessential/bestool/commit/29162988784b913a462ce89fe7c9f0d647f77723))

- **tweak(aws):** Opt into 2024 behaviour (stalled stream protection for uploads) - ([53f428a](https://github.com/beyondessential/bestool/commit/53f428aeb57c32538fc219125efa2102904d238b))

- **doc(backup):** Fix help for trailing args - ([95df135](https://github.com/beyondessential/bestool/commit/95df1350df8b1d136f5cce9a45aa883b1c0951bc))
- **feat(backup):** KAM-297: add ability to encrypt backups (#174) - ([f7c77af](https://github.com/beyondessential/bestool/commit/f7c77afda4c586c9178d5d5c864582272238e65b))
- **test(backup):** Update snapshot - ([4abcfc1](https://github.com/beyondessential/bestool/commit/4abcfc10238e83f940a4e837f3d752d6f18a04b1))

- **feat(backups):** Add --keep-days option to cleanup old backups - ([232336b](https://github.com/beyondessential/bestool/commit/232336b2bb0ed46c6ac11f6aae26fd550f803076))
- **feat(backups):** Add encryption and --then-copy-to and --keep-days to config backups - ([7c7b66e](https://github.com/beyondessential/bestool/commit/7c7b66e8760c790efcf1075c77b618f4c74a3e6b))
- **feat(backups):** Create dest dir and fix log output - ([f5794c2](https://github.com/beyondessential/bestool/commit/f5794c25f8712e267ff687570a906c8ab05c3df2))
- **feat(backups):** Use zip for configs instead of tar - ([3bc7852](https://github.com/beyondessential/bestool/commit/3bc7852c13e4880a0abf70cc1459d857de1390ba))
- **feat(backups):** Create dir for --then-copy-to - ([fbb72d5](https://github.com/beyondessential/bestool/commit/fbb72d5bf7705d154ddf7210abb8db8264a88a31))
- **fix(backups):** Compute output filename properly - ([10e6bae](https://github.com/beyondessential/bestool/commit/10e6baeb4b3d1eac100d50b34e5a8f48d5f793bc))
- **fix(backups):** Don’t nest backup in duplicate folders when splitting - ([0d3abe0](https://github.com/beyondessential/bestool/commit/0d3abe057898ab01b5b21d39e4bf08828fc9bd69))
- **test(backups):** Can't do deterministic zips - ([29a1f02](https://github.com/beyondessential/bestool/commit/29a1f0281b4135b99fb329e8855f1028ee66fa97))
- **test(backups):** Remove --deterministic - ([8f88348](https://github.com/beyondessential/bestool/commit/8f8834885d0d8adab8c47ab0babc099676530eae))
- **tweak(backups):** Do file copy in Rust to get a progress indication - ([75ccf63](https://github.com/beyondessential/bestool/commit/75ccf6397a81adf7f066afd43c14345fadb14920))
- **tweak(backups):** Use zero-compression zips - ([bc3e064](https://github.com/beyondessential/bestool/commit/bc3e0640473a18b994632b306117c75a3fc9b5c4))
- **tweak(backups):** Use filesystem copy if we can - ([e96eec0](https://github.com/beyondessential/bestool/commit/e96eec03018271da06210694e512bd903152b3d2))
- **tweak(backups):** Only exclude non-critical log tables (#210) - ([6447300](https://github.com/beyondessential/bestool/commit/6447300e3030c2809a7aa51e657f3c7f7c971edb))

- **feat(bestool):** Download from tailscale proxies when available - ([c8fa0ab](https://github.com/beyondessential/bestool/commit/c8fa0abd8ae498fb090a4b8bcc840847b6838793))

- **feat(cli):** Enable unambiguous shorthands - ([312cca9](https://github.com/beyondessential/bestool/commit/312cca9dada2c769235758c8efa6767b3fd2eca7))

- **doc(crypto):** Explain how to use the identity file in keygen - ([f7c77af](https://github.com/beyondessential/bestool/commit/f7c77afda4c586c9178d5d5c864582272238e65b))
- **doc(crypto):** Fix description of keygen - ([374d4ae](https://github.com/beyondessential/bestool/commit/374d4aeae0c6d4e7b7a600c9490399400c7c517e))
- **feat(crypto):** KAM-297: add encrypt, decrypt, and keygen (#169) - ([a5367c3](https://github.com/beyondessential/bestool/commit/a5367c3c239045ea09a4336e3308fbd64d1bcddf))
- **feat(crypto):** Add protect/reveal commands for passphrase encryption - ([75f8e1d](https://github.com/beyondessential/bestool/commit/75f8e1d35aa01e27099822d0e77a96e75701317f))
- **feat(crypto):** Encrypt identity files by default - ([84061c3](https://github.com/beyondessential/bestool/commit/84061c307c028996fb5222d696cc7f569687363c))
- **feat(crypto):** Support encrypted identity files directly while en/decrypting - ([abc86a8](https://github.com/beyondessential/bestool/commit/abc86a8464820905a814215d9afa11b37b61eea6))
- **feat(crypto):** Add --rm to encrypt and protect - ([8828421](https://github.com/beyondessential/bestool/commit/88284210a5fa0ad2dce97f208248b9ab4adbcc70))
- **feat(crypto):** Write identity.pub by default - ([a39d39d](https://github.com/beyondessential/bestool/commit/a39d39d67f7e8ad7c7777cfc1732471c1ee249a9))
- **refactor(crypto):** Remove minisign subcommands - ([4cb17c6](https://github.com/beyondessential/bestool/commit/4cb17c608b00b420818228c0ac143293c1227ecb))
- **refactor(crypto):** Extract en/decryption and key handling routines - ([f7c77af](https://github.com/beyondessential/bestool/commit/f7c77afda4c586c9178d5d5c864582272238e65b))
- **refactor(crypto):** Use algae-cli in bestool - ([347af7a](https://github.com/beyondessential/bestool/commit/347af7ac6e05fdc50005abd5ca70eb5ae7a89a88))

- **fix(db-url):** Handle the case where a reporting username is empty in config - ([8af528a](https://github.com/beyondessential/bestool/commit/8af528a6a71e2630a10b154d77aad7c4e11f6fd5))

- **fix(downloads):** Use full tailscale name for alternative sources - ([ebb6522](https://github.com/beyondessential/bestool/commit/ebb6522d90c0af3837f2b166c0524a0434457f46))
- **fix(downloads):** Query tailscale dns directly to avoid buggy systems - ([63dccad](https://github.com/beyondessential/bestool/commit/63dccadcd8a2b2efe6ee14e57d19e356e0ef59eb))

- **feat(greenmask):** Support multiple config directories - ([c02a898](https://github.com/beyondessential/bestool/commit/c02a8980b33159a04510e13f987598a5b9645e02))
- **feat(greenmask):** Look into release folder by default too - ([26ab927](https://github.com/beyondessential/bestool/commit/26ab9278786e562a404818137d79d6412a04a52d))
- **feat(greenmask):** Create storage dir if missing - ([1ddc568](https://github.com/beyondessential/bestool/commit/1ddc568f63a147a50f000dbb778b2755d954fc16))
- **fix(greenmask):** Default all paths - ([fc30309](https://github.com/beyondessential/bestool/commit/fc303094694e37b9d692a2cf4ab94bbe678ca60c))
- **fix(greenmask):** Correct storage stanza - ([604d184](https://github.com/beyondessential/bestool/commit/604d18416441c801bc6cd68013017d1646597451))
- **fix(greenmask):** Use dunce canonicalize instead of unc - ([476844f](https://github.com/beyondessential/bestool/commit/476844f835832ea1cdb789dd4392eae856854a02))

- **feat(iti):** Add systemd services for lcd display (#75) - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **feat(iti):** Add temperature to lcd - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **feat(iti):** Add local time to lcd - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **feat(iti):** Add network addresses to lcd - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **feat(iti):** Add wifi network to lcd - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **feat(iti):** Sparklines for cpu/ram usage - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **fix(iti):** Properly clear lcd on start and stop - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **refactor(iti):** Simplify bg/fg colour calculations - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **refactor(iti):** Remove wifisetup wip command - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **refactor(iti):** Pass upper args through - ([8e1837c](https://github.com/beyondessential/bestool/commit/8e1837cfb82c04de915ace644cf1e0399a56ae46))
- **tweak(iti):** Make time less precise for battery display - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **tweak(iti):** More responsive battery display - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **tweak(iti):** Add fully charged message - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))

- **feat(psql):** Add read-only mode - ([fee043c](https://github.com/beyondessential/bestool/commit/fee043c29a3d85ed8acf76a59f9c37cd070da72a))
- **feat(psql):** Invert read/write default, require -W, --write to enable write mode - ([d947eb6](https://github.com/beyondessential/bestool/commit/d947eb65130e1b004507e33e877069fa48be487b))
- **feat(psql):** Arbitrary program and args - ([efed90a](https://github.com/beyondessential/bestool/commit/efed90a4431058e0efe230f43b88ee883a4f160e))
- **feat(psql):** Syntax highlighting - ([67e93d4](https://github.com/beyondessential/bestool/commit/67e93d4c10f3aad2aeb034136ecea5e72da6d186))
- **tweak(psql):** Turn autocommit off when -W is given - ([b7636ce](https://github.com/beyondessential/bestool/commit/b7636ce3b197fb2301d329a9ce74db90ccfb3eb8))
- **tweak(psql):** Use UTF-8 codepage on Windows and force UTF8 encoding on PSQL - ([797fb83](https://github.com/beyondessential/bestool/commit/797fb835e66c5522f0c111aaa292474c4b253b44))
- **tweak(psql):** Default to \timing on - ([68843f2](https://github.com/beyondessential/bestool/commit/68843f2ab3c74acb7c1abce83e805ea8aad5df8b))
- **tweak(psql):** Use reporting users when present - ([0bcb91f](https://github.com/beyondessential/bestool/commit/0bcb91f10b457aa9bc5ae7d12ad66acb12511542))
- **tweak(psql):** Allow customising the codepage on windows - ([ffd3aff](https://github.com/beyondessential/bestool/commit/ffd3aff57c5b26450c68662b3a4ec47ceb15ab5c))

- **feat(self-update):** Add ourselves to PATH on windows with -P - ([c5e4651](https://github.com/beyondessential/bestool/commit/c5e4651628215960175a57ff7837810c0c18785e))

- **doc(tamanu):** Fix docstring for tamanu download - ([4b36867](https://github.com/beyondessential/bestool/commit/4b368678c444d48aa671d1673e927aad881de603))
- **feat(tamanu):** Add greenmask-config command - ([4e874de](https://github.com/beyondessential/bestool/commit/4e874de4c1c61ae9c3c0ebd082d0a430bfc07eca))
- **feat(tamanu):** Add postgres backup tool (#137) - ([4f4c549](https://github.com/beyondessential/bestool/commit/4f4c549c3c996911d19a15b348e1562970fb07fd))
- **feat(tamanu):** Command to list artifacts from meta (#212) - ([d5ead2e](https://github.com/beyondessential/bestool/commit/d5ead2e87f7b0abbfac4fa4f1e55268d9cc6bcf1))
- **feat(tamanu):** Remove unused upgrade and pre-upgrade commands - ([d5ead2e](https://github.com/beyondessential/bestool/commit/d5ead2e87f7b0abbfac4fa4f1e55268d9cc6bcf1))
- **feat(tamanu):** Add dburl command - ([7eb2c90](https://github.com/beyondessential/bestool/commit/7eb2c9079d7393d6ee2b09f0d95b880e927e0ef6))
- **feat(tamanu):** Use new psql tool - ([fb5b6ae](https://github.com/beyondessential/bestool/commit/fb5b6ae49dbdb5b9093cb6b2810a19aab0bab777))
- **fix(tamanu):** Look into the right places for Linux installs' config - ([7b87111](https://github.com/beyondessential/bestool/commit/7b871115a518b12acc1a4ec2d64683ac6e3c00eb))
- **fix(tamanu):** Assume facility if we can't detect server type - ([78f605e](https://github.com/beyondessential/bestool/commit/78f605ef92b8edb8962f49ce6c850c591edd5e14))
- **fix(tamanu):** Windows compilation - ([12fdd8b](https://github.com/beyondessential/bestool/commit/12fdd8b3a651d4f42c06192ba399be05166c3631))
- **fix(tamanu):** Do not require mailgun until needed - ([99496d2](https://github.com/beyondessential/bestool/commit/99496d258b921892c1fcf439bd55275ecf01057d))
- **tweak(tamanu):** Make download command able to download any artifact - ([d5ead2e](https://github.com/beyondessential/bestool/commit/d5ead2e87f7b0abbfac4fa4f1e55268d9cc6bcf1))
- **tweak(tamanu):** Support port field - ([2407714](https://github.com/beyondessential/bestool/commit/24077145d5ff64153fd7abc83a378efc5913b79d))
- **tweak(tamanu):** Move dburl command to url for mnemonics - ([e38cfcc](https://github.com/beyondessential/bestool/commit/e38cfccc0dae4b30f371c56a3306a6a1687e8b2e))

- **tweak(url):** Don't include empty password if no password is provided - ([9117dd5](https://github.com/beyondessential/bestool/commit/9117dd5a32231002f00c577b367ad1deaa3185ac))
<!-- generated by git-cliff -->
