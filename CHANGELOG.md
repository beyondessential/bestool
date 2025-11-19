# Changelog

All notable changes to this project will be documented in this file.

See [conventional commits](https://www.conventionalcommits.org/) for commit guidelines.

---
## [unreleased]




- **feat(self-update):** Check that we can write the exe before attempting - ([631c9e4](https://github.com/beyondessential/bestool/commit/631c9e43648af17e535509cc70d52bcbeeda0c7f))
---
## [1.1.15](https://github.com/beyondessential/bestool/compare/v1.1.14..v1.1.15) - 2025-11-18




- **fix(self-update):** Delete temp file before downloading update - ([08b2102](https://github.com/beyondessential/bestool/commit/08b2102559562d5a3d911f67c06b0fa3042effe6))
---
## [1.1.14](https://github.com/beyondessential/bestool/compare/v1.1.13..v1.1.14) - 2025-11-18


- **deps:** Transitives - ([3d8ad0d](https://github.com/beyondessential/bestool/commit/3d8ad0d9f94aa1ed573ea26b44b7febcf87dcaa5))


- **feat(psql):** Support multiple commands in one input - ([b0a5c81](https://github.com/beyondessential/bestool/commit/b0a5c81d6e03ee573c2463020c8be4381c491b26))
- **feat(psql):** Support alt-enter to input newlines - ([0a35802](https://github.com/beyondessential/bestool/commit/0a358020491e38068bbb89799f04182830660f6b))
- **fix(psql):** Deprecations - ([70400a5](https://github.com/beyondessential/bestool/commit/70400a53c2f3a01842eb4322a8493320629e2b77))
- **fix(psql):** Misc completion bugs - ([0188d32](https://github.com/beyondessential/bestool/commit/0188d32c79157261d5a697bece0319e73e357fed))
- **tweak(psql):** Handle comments with metacommands - ([79af71b](https://github.com/beyondessential/bestool/commit/79af71b32e628950fb3ec09875ac719d3297a913))
- **tweak(psql):** Generate modifier completions - ([68b11bd](https://github.com/beyondessential/bestool/commit/68b11bdcdf5261b7b3658c4c6e73f3f8b9c0d79d))
- **tweak(psql):** Autocomplete the \g - ([2350e23](https://github.com/beyondessential/bestool/commit/2350e231baaaf56d38ddf400a621d5849aa50485))
---
## [1.1.13](https://github.com/beyondessential/bestool/compare/v1.1.12..v1.1.13) - 2025-11-17


- **deps:** Switch reqwest to rustls - ([0893484](https://github.com/beyondessential/bestool/commit/089348402854911ed664a293749c62dead35d34b))
- **deps:** Transitives - ([04e8866](https://github.com/beyondessential/bestool/commit/04e8866c2f149c1b093062c28360bdb5cff00945))
- **deps:** Bump the deps group across 1 directory with 3 updates (#240) - ([7eb3b60](https://github.com/beyondessential/bestool/commit/7eb3b6034915d92bded1ec2b9fbfb5942f2b84bd))
- **fix:** Fix for windows - ([e22e43a](https://github.com/beyondessential/bestool/commit/e22e43afa09ffa69c9e863b4e359983930c38e62))


- **feat(psql):** Support unix sockets - ([ef3ca49](https://github.com/beyondessential/bestool/commit/ef3ca49022f9b3939fea141483660ffc62924742))
- **feat(psql):** Show rows affected instead of (no rows) for inserts etc - ([5891043](https://github.com/beyondessential/bestool/commit/5891043e68f388b5167aec732796875417d83df1))
- **fix(psql):** Special-case /var/sock-like database host - ([b355db6](https://github.com/beyondessential/bestool/commit/b355db664ebf96ba97a1ec2c8dc44c7465cedbab))
- **fix(psql):** Stop re-querying when having to print things we can't cast to - ([a046739](https://github.com/beyondessential/bestool/commit/a046739ca9b6b3bc022adbcccb596a6b6492e109))
- **tweak(psql):** Add guidance for \copy - ([2bd9a00](https://github.com/beyondessential/bestool/commit/2bd9a00de3acb081054a9bf04bc854cfccd9dd19))
---
## [1.1.12](https://github.com/beyondessential/bestool/compare/v1.1.11..v1.1.12) - 2025-11-10


- **deps:** Transitives - ([4da68dd](https://github.com/beyondessential/bestool/commit/4da68dd43f90844730e12a2101fa293daeb37278))


- **fix(psql):** Fix the non-encoded url issue - ([9c2ec21](https://github.com/beyondessential/bestool/commit/9c2ec216781c24ce9665bffaf4b26042a01e616e))
- **tweak(psql):** More detail on connection errors - ([f846ce1](https://github.com/beyondessential/bestool/commit/f846ce1bed19f18e5a740f165d62c4137da84449))
---
## [1.1.11](https://github.com/beyondessential/bestool/compare/v1.1.10..v1.1.11) - 2025-11-04




- **fix(psql):** No longer update schema cache every 60s to avoid overwhelming - ([0fbc485](https://github.com/beyondessential/bestool/commit/0fbc485cd78d558b0720f1c1c1247416c11dd40e))
- **perf(psql):** Run schema cache queries in parallel with a 15 second - ([f5b80aa](https://github.com/beyondessential/bestool/commit/f5b80aa91ec770f666fcd41eee732c92d01419bb))

- **tweak(tamanu/psql):** Log the url for debugging - ([8d231ea](https://github.com/beyondessential/bestool/commit/8d231eab89b9e75b0647b47887f02837059e2f9a))
---
## [1.1.10](https://github.com/beyondessential/bestool/compare/v1.1.9..v1.1.10) - 2025-11-02




- **fix(psql):** Checkpoint sqlite database to only have the single file - ([7a2effc](https://github.com/beyondessential/bestool/commit/7a2effcf00534316e4afc37d0ed115391f240af4))
- **tweak(psql):** Refuse to overwrite files - ([3c2a268](https://github.com/beyondessential/bestool/commit/3c2a268215843cde7c357a3f67372c07a81763e0))
---
## [1.1.9](https://github.com/beyondessential/bestool/compare/v1.1.8..v1.1.9) - 2025-11-02




- **fix(psql):** Fix locking issues on Windows - ([260702c](https://github.com/beyondessential/bestool/commit/260702c6e04c625cc77d4ea3fc4a42e856dec191))
---
## [1.1.8](https://github.com/beyondessential/bestool/compare/v1.1.7..v1.1.8) - 2025-11-02


- **doc:** Fix mistakes in examples - ([733a9e3](https://github.com/beyondessential/bestool/commit/733a9e3b189418165268eaaa228c879673485bb4))
- **doc:** Mention \gzset explicitly - ([a96195f](https://github.com/beyondessential/bestool/commit/a96195fe3f1d8a8691d17aac8c8fc1e38e235b49))
- **feat:** Replace rusqlite with turso (pure Rust) (#234) - ([fa40045](https://github.com/beyondessential/bestool/commit/fa40045ec1697e972be53f11e3e04cbc0c75046e))
- **feat:** Replace rusqlite with turso (pure Rust) - ([1a57415](https://github.com/beyondessential/bestool/commit/1a57415938bbd053461d2228d8d761c726f4b43b))


- **doc(psql):** Add csv to the examples - ([e6269b1](https://github.com/beyondessential/bestool/commit/e6269b14007a66e4ca8b40f06351eb7950f376b4))
- **doc(psql):** Audit db isn't actually usable from multiple processes - ([bf69289](https://github.com/beyondessential/bestool/commit/bf692897948b3513c24c660daaf2caa0300b686f))
- **feat(psql):** Add csv \re show format - ([7b2c46e](https://github.com/beyondessential/bestool/commit/7b2c46e165a00a8fdad53c8559d10f346c5c6b23))
- **feat(psql):** Add sqlite and excel export formats - ([7b4bd15](https://github.com/beyondessential/bestool/commit/7b4bd15cc7765cfb16325490482558e69f174f01))
- **feat(psql):** Audit db multi-process support - ([e584e13](https://github.com/beyondessential/bestool/commit/e584e1389d281c5eff8240371263179e004f9f5d))
- **feat(psql):** Save the instance id to audit log - ([e392ea2](https://github.com/beyondessential/bestool/commit/e392ea20712b879186110fd61a6dd64939ba6ad7))
- **feat(psql):** Add audit export command - ([453bea3](https://github.com/beyondessential/bestool/commit/453bea3893c9ebca14db652d5cccf395b1956f62))
- **fix(psql):** Crash and exit handling - ([88e65dd](https://github.com/beyondessential/bestool/commit/88e65dd1cb5a0372b39345b46740bac26a6d899c))
- **fix(psql):** Don't crash if we can't cull the database - ([049e6f4](https://github.com/beyondessential/bestool/commit/049e6f4dd0546732b2aaeed0c2d15c62db4b5876))
- **fix(psql):** Tests - ([7626fcf](https://github.com/beyondessential/bestool/commit/7626fcf62ec77de78a85433eab8a78f04567dd3e))
- **perf(psql):** Store history-timestamp index on disk - ([b8c2d0a](https://github.com/beyondessential/bestool/commit/b8c2d0a0bb8a6392ba1589e5dfd3c30fa8e991d6))
- **tweak(psql):** --audit-path specifies directory, not file - ([bdb9d1e](https://github.com/beyondessential/bestool/commit/bdb9d1e6a2d1c9444b52574087774fbb26a9e142))
---
## [1.1.7](https://github.com/beyondessential/bestool/compare/v1.1.6..v1.1.7) - 2025-11-01




- **fix(psql):** Always save the last result, even if it exceeds max size - ([0606902](https://github.com/beyondessential/bestool/commit/06069022078827584a2bbac00d86e4f9b7386b6a))
- **tweak(psql):** Warn on low memory - ([7e0f2ff](https://github.com/beyondessential/bestool/commit/7e0f2ffef0c3f5ad6fea7fbc137d491b1040d7e9))
---
## [1.1.6](https://github.com/beyondessential/bestool/compare/v1.1.5..v1.1.6) - 2025-11-01




- **fix(psql):** Don't crash if we can't handle table info with \d - ([975338e](https://github.com/beyondessential/bestool/commit/975338e390429a044fa0a232e43409aa3afefbfc))
- **fix(psql):** Cast regclass to text to actually fix issues - ([18e947d](https://github.com/beyondessential/bestool/commit/18e947deca387f4e60c01cbe2b96fa57b65db025))
- **tweak(psql):** Hide referenced-by and triggers in \d+ for tables - ([4aafdda](https://github.com/beyondessential/bestool/commit/4aafddab23a41857a25babb1757bc8893e26b5b4))
---
## [1.1.5](https://github.com/beyondessential/bestool/compare/v1.1.4..v1.1.5) - 2025-11-01


- **doc:** Update example - ([424e2ae](https://github.com/beyondessential/bestool/commit/424e2aeb5763dcda8b6f8f759207063b6e3e646d))


- **tweak(psql):** In json outputs, respect column order - ([2ff6dc8](https://github.com/beyondessential/bestool/commit/2ff6dc846ea3de7622d97f23533829c995982ffd))
---
## [1.1.4](https://github.com/beyondessential/bestool/compare/v1.1.3..v1.1.4) - 2025-11-01




- **feat(psql):** Refuse to exit while in active transaction - ([6b6e8f4](https://github.com/beyondessential/bestool/commit/6b6e8f4987d3f7d190a3588e97448484864c7cbc))
- **feat(psql):** Show a list of saved results - ([aabfb25](https://github.com/beyondessential/bestool/commit/aabfb25c8b49450ec1ee229a98009e6d0854a71e))
- **feat(psql):** \re show command - ([05ed0a4](https://github.com/beyondessential/bestool/commit/05ed0a4e62a9ef254ff3e6570457f7134383ddb4))
- **feat(psql):** \gz for zero output and auto-truncation - ([f4150cf](https://github.com/beyondessential/bestool/commit/f4150cf223ac6617a01dd45b1a3c785bdb9dcbef))
- **feat(psql):** Show an indicator when a long query is running - ([f2b10d6](https://github.com/beyondessential/bestool/commit/f2b10d64b84a4737b27772a96574c7130aca7e73))
- **feat(psql):** Handle void and test for numeric - ([b88f6e4](https://github.com/beyondessential/bestool/commit/b88f6e4563c9d9ad3563e936a216f4d16f25edab))
- **feat(psql):** Improved numeric handling - ([a802457](https://github.com/beyondessential/bestool/commit/a80245757dba082e6ccce2b1e0de0fa89e7e7f84))
---
## [1.1.3](https://github.com/beyondessential/bestool/compare/v1.1.2..v1.1.3) - 2025-11-01




- **fix(psql):** Style the prompt the right way - ([841ffdc](https://github.com/beyondessential/bestool/commit/841ffdcecb57d2ab5c21e491affeebeb8ad85608))
---
## [1.1.2](https://github.com/beyondessential/bestool/compare/v1.1.1..v1.1.2) - 2025-11-01


- **deps:** Uzers is only for unix - ([8281df1](https://github.com/beyondessential/bestool/commit/8281df1d1d11569d9c7bff38aaa47efa699a2513))


- **feat(psql):** Pretty error rendering - ([7606033](https://github.com/beyondessential/bestool/commit/7606033ee9d7c9e1acab1b3505c2b53c4596796b))
- **fix(psql):** Mobc errors aren't tokio errors - ([a5036d7](https://github.com/beyondessential/bestool/commit/a5036d735729f28300c652d5617ead74bf19a272))

- **fix(ssh):** Windows deps again - ([2b36eaa](https://github.com/beyondessential/bestool/commit/2b36eaa247be87e04032b01e0a7cbe14fb3a7c74))
---
## [1.1.1](https://github.com/beyondessential/bestool/compare/v1.1.0..v1.1.1) - 2025-11-01


- **deps:** Replace unsafe serde-yml with unmaintained-but-safe serde-yaml - ([b76d8ec](https://github.com/beyondessential/bestool/commit/b76d8ec8480288f27365a02d16ba6ef869d17762))
- **deps:** Replace unsound users with maintained uzers - ([d76d720](https://github.com/beyondessential/bestool/commit/d76d720df28a94ebea5a25d972bac6f054f36791))
- **deps:** Misc updates - ([e12b224](https://github.com/beyondessential/bestool/commit/e12b224f24af68efba1b3efee57add865208a3dc))
- **repo:** Fix typos config - ([23301f3](https://github.com/beyondessential/bestool/commit/23301f3c767bbd84f1abf88b077620bfd07b2b0d))


- **fix(psql):** Support postgres 13 for \describe table - ([cf698f8](https://github.com/beyondessential/bestool/commit/cf698f8ab8e7d447c4f9ce86011a1a6d591a7a66))
- **fix(psql):** Use non-printing markers for ansi styles - ([278d9e2](https://github.com/beyondessential/bestool/commit/278d9e290c1d2356f3df944a23b8614551024ba8))
---
## [1.1.0](https://github.com/beyondessential/bestool/compare/v1.0.13..v1.1.0) - 2025-11-01


- **deps:** Update transitives - ([d2fcabb](https://github.com/beyondessential/bestool/commit/d2fcabb1ab7a864531312c576833692cf0226c93))
- **doc:** Add variable interpolation help text - ([723378f](https://github.com/beyondessential/bestool/commit/723378f408c5d543665bdd2cf6c1651ae7faf7cf))
- **doc:** Add \snip run and \snip save to help output - ([ebe2f9a](https://github.com/beyondessential/bestool/commit/ebe2f9aa427780cdc14f93115d81bfa88938553d))
- **doc:** Add comprehensive README.md to psql2 - ([520e945](https://github.com/beyondessential/bestool/commit/520e94593e6a8652060a5062ffce775d71d95591))
- **feat:** Add variable interpolation syntax to queries - ([651cbae](https://github.com/beyondessential/bestool/commit/651cbaef4eb58068fa3904711f1ee952d12bad90))
- **feat:** Add Verbatim (v) query modifier to skip variable interpolation - ([5b4826d](https://github.com/beyondessential/bestool/commit/5b4826d9de52ab0e675f95fb86ea6bb6ba5833a9))
- **feat:** Add cross-platform Ctrl-C query cancellation support - ([2ccd2e4](https://github.com/beyondessential/bestool/commit/2ccd2e4d4fa1cd050e284713664152b8d0cd899c))
- **feat:** Implement SnippetSave metacommand to save preceding command - ([3ed6e09](https://github.com/beyondessential/bestool/commit/3ed6e0930df886cf4a897e509c947ecd0eec747a))
- **feat:** Auto-create savedir in Snippets::save and add comprehensive tests - ([ee32265](https://github.com/beyondessential/bestool/commit/ee32265af5d11783fa0d9c25552ae9558af79e31))
- **feat:** Add snippet name completion for \snip run and \snip save - ([1323826](https://github.com/beyondessential/bestool/commit/1323826fe2dfa1982b938a5d0f6c21ef1989e4b3))
- **feat:** Add variable arguments to \snip run and \i commands - ([9aa969d](https://github.com/beyondessential/bestool/commit/9aa969dc3d87b7d60958a5d6656186a61bc634af))
- **feat:** Print snippet filename when saving - ([507789d](https://github.com/beyondessential/bestool/commit/507789d75d88e2dcc67598e1cb567f800df4dc26))
- **feat:** Restrict completion for \snip to only subcommands - ([9bd7755](https://github.com/beyondessential/bestool/commit/9bd7755f152f3146cec49505fcb4cfab063f0d5f))
- **feat:** Support executing multiple SQL statements separated by semicolons - ([6574d8a](https://github.com/beyondessential/bestool/commit/6574d8a83014f194ebb01e6170be31f91f10f8d0))
- **fix:** Exclude SnippetSave from initial history, add after save completes - ([7d3317c](https://github.com/beyondessential/bestool/commit/7d3317cfb37ac0835190c7385e253e792bde77d4))
- **fix:** Always add SnippetSave to history, even on error - ([a0da33c](https://github.com/beyondessential/bestool/commit/a0da33cb0572190f26afbaca9e6e5ac05ba1369f))
- **fix:** Resolve all clippy warnings in psql2 crate - ([00e4c28](https://github.com/beyondessential/bestool/commit/00e4c28a32aa5310f84ebeaf66caac9feab33f28))
- **refactor:** Make Snippets::save return the path it wrote to - ([6e07637](https://github.com/beyondessential/bestool/commit/6e07637aefe666d7fcebe3264b4b7e5d5eb49033))
- **refactor:** Use comfy-table for help output formatting - ([6b973d9](https://github.com/beyondessential/bestool/commit/6b973d97c71ebb1d65231e5ce88eb5e888d90603))
- **refactor:** Use configure_table for help output tables - ([0aaf7cc](https://github.com/beyondessential/bestool/commit/0aaf7cc3782fcf415ec2fc40bac8efd87b2778d4))
- **refactor:** Use new format style {var} in macros - ([9757d70](https://github.com/beyondessential/bestool/commit/9757d709a5ef24b096a8ec9c68986a0624853597))
- **refactor:** Fix imports to use merged style and proper grouping - ([68dc19b](https://github.com/beyondessential/bestool/commit/68dc19bee7c5ede5b193f2fb32e8b14efa9d9bdf))
- **refactor:** Split repl, completer, parser, and query into submodules - ([5a3a98a](https://github.com/beyondessential/bestool/commit/5a3a98ac51011cea3b0d4fcdfeac0ac32daeb10c))
- **repo:** Add minimal .rules - ([fe32a64](https://github.com/beyondessential/bestool/commit/fe32a6449023dcbe955ded312d1c71051cbe4500))
- **revert:** Remove JSON syntax highlighting due to table width issues - ([7788930](https://github.com/beyondessential/bestool/commit/77889304648a3819aa2469815bde4904419d2b1e))
- **test:** Add unit tests for Verbatim modifier behavior - ([92fa7af](https://github.com/beyondessential/bestool/commit/92fa7afc85aa073dc9ad98dddd8ebb9cb15827b3))
- **tweak:** Tweak rules - ([ea8d6f8](https://github.com/beyondessential/bestool/commit/ea8d6f8b7985036637615c95231f0f280c4dff6c))


- **doc(psql):** Write full readme - ([7bb2f3d](https://github.com/beyondessential/bestool/commit/7bb2f3defeb57e8da1eac6e5a16c76065a513fee))
- **doc(psql):** Mention verbatim mode - ([990403d](https://github.com/beyondessential/bestool/commit/990403d74908291c3c188ca2142f0b3b492af55d))
- **feat(psql):** Integrate new psql2 into bestool - ([fee167d](https://github.com/beyondessential/bestool/commit/fee167d7bab5fd93920f6924cdf07a7a5af77ff6))

- **doc(psql2):** Add comment about comfy-table ANSI handling - ([e20c0a5](https://github.com/beyondessential/bestool/commit/e20c0a521f15c3cd35bbec3c40770deb762c0e6a))
- **doc(psql2):** Add \list and \dt commands to help text - ([61008e9](https://github.com/beyondessential/bestool/commit/61008e9ddbce02c0aa6a9de8e4dfcdb1d4d3922c))
- **feat(psql2):** Create new crate with basic frame - ([7967e2b](https://github.com/beyondessential/bestool/commit/7967e2b88cce68c7d1064a8d0161ed66a6d932be))
- **feat(psql2):** Add async postgres client with version query - ([2729367](https://github.com/beyondessential/bestool/commit/27293670899724fdd3bc6a50569c5a40ade49977))
- **feat(psql2):** Add rustyline-based REPL - ([569ae5c](https://github.com/beyondessential/bestool/commit/569ae5c7bc36c35372d6597ae0aaadf359867379))
- **feat(psql2):** Add SQL syntax highlighting - ([2f5b192](https://github.com/beyondessential/bestool/commit/2f5b192aeba8491797063914558971a4f98a738f))
- **feat(psql2):** Default to localhost connection for simple dbname - ([71bc9ae](https://github.com/beyondessential/bestool/commit/71bc9aeca1f9df49c8cef73e0244eacca6e43735))
- **feat(psql2):** Use cli-table for formatted query results - ([536bf23](https://github.com/beyondessential/bestool/commit/536bf230378c1681cce0f3089cbd9d189aac3da8))
- **feat(psql2):** Switch to comfy-table with UTF8 rounded corners - ([ebc5009](https://github.com/beyondessential/bestool/commit/ebc5009f9e8daf5a6dcb2ed48a6eafe66a49ba52))
- **feat(psql2):** Make table headers bold and center-aligned - ([693aece](https://github.com/beyondessential/bestool/commit/693aecea22a64a4a6f17e62ce4a2014c35f496f4))
- **feat(psql2):** Add history tracking with redb - ([264d72a](https://github.com/beyondessential/bestool/commit/264d72afd022705c56805633fbcc78116391f772))
- **feat(psql2):** Add comprehensive type handling for PostgreSQL built-in types - ([c60ef27](https://github.com/beyondessential/bestool/commit/c60ef27119ec41433e999aa024c95968f29ec061))
- **feat(psql2):** Add support for PostgreSQL array types - ([4e32cb0](https://github.com/beyondessential/bestool/commit/4e32cb07b956a71506c42973974bb2e0b32e9fd6))
- **feat(psql2):** Add text fallback for composite/record types using ::text cast - ([8fa21db](https://github.com/beyondessential/bestool/commit/8fa21db716fbdaa535a670b73dc40ca81e838b43))
- **feat(psql2):** Require trailing semicolon or \g to execute, add multiline editing - ([a0ba2a8](https://github.com/beyondessential/bestool/commit/a0ba2a8292d6d55c6f96aacac22f0f33eccb0777))
- **feat(psql2):** Show database name in prompt and # for superusers - ([2db32f5](https://github.com/beyondessential/bestool/commit/2db32f56bd1b182ba8c20f99590505102707beba))
- **feat(psql2):** Add JSON syntax highlighting in result rows - ([d0a1694](https://github.com/beyondessential/bestool/commit/d0a169406d1810330bcbe9937f1bbdb53c6fed57))
- **feat(psql2):** Support \g as query terminator and show execution time - ([ac67a6e](https://github.com/beyondessential/bestool/commit/ac67a6ee6fb48a85fbfc768c0dc7e0e2a2f4c640))
- **feat(psql2):** Add support for \gx, \gset, and \gxset metacommands - ([ce9c812](https://github.com/beyondessential/bestool/commit/ce9c812d5654085a4d66bda766556d90d7acd3ba))
- **feat(psql2):** Refactor parser to handle modifiers as sequence, add json modifier - ([579ec01](https://github.com/beyondessential/bestool/commit/579ec019a30be7e5000f37e444cb7cb3734609da))
- **feat(psql2):** Add active modifiers to query execution debug log - ([1bf538d](https://github.com/beyondessential/bestool/commit/1bf538d78e3a3188fd0f7c976d44ae07d7275578))
- **feat(psql2):** Add completer - ([ea063a4](https://github.com/beyondessential/bestool/commit/ea063a42106c7b43d66f83f5ee1c24e81f33b4bc))
- **feat(psql2):** Add TLS support and write mode with OTS - ([7c7708e](https://github.com/beyondessential/bestool/commit/7c7708e2e54316cbe49dbd220c74efb07644368e))
- **feat(psql2):** Disable autocommit and show transaction state in prompt - ([109da40](https://github.com/beyondessential/bestool/commit/109da408df1a8902d4cf111a7b6fa00fa4053546))
- **feat(psql2):** Add colored prompts based on write mode and transaction state - ([5a8e9ef](https://github.com/beyondessential/bestool/commit/5a8e9ef156178061f06e4eb2ecadde0a030a0fd5))
- **feat(psql2):** Expanded mode - ([5aecb59](https://github.com/beyondessential/bestool/commit/5aecb59c79a5f4ec77e0f463463f5069871aa1d9))
- **feat(psql2):** Json modes - ([c8c2d58](https://github.com/beyondessential/bestool/commit/c8c2d58937f0d7a1432880adad99718eb94a0286))
- **feat(psql2):** Add metacommand parser - ([88c4635](https://github.com/beyondessential/bestool/commit/88c4635c74832f4ca1427f5223e22ef3a1416c3c))
- **feat(psql2):** Add expanded toggle - ([463df68](https://github.com/beyondessential/bestool/commit/463df68f81b982668e575e66407bff3898470d1b))
- **feat(psql2):** Add \W metacommand for write mode toggle - ([e00a39d](https://github.com/beyondessential/bestool/commit/e00a39da8bfc022c6b238068a894bbab42328b83))
- **feat(psql2):** Respect terminal width and wrap long content in table output - ([18165ef](https://github.com/beyondessential/bestool/commit/18165ef90bf3e2004f1f9ebfb3c3a8146cb02f40))
- **feat(psql2):** Handle ctrl-c during query execution to cancel queries gracefully - ([615c823](https://github.com/beyondessential/bestool/commit/615c823120db5f9e1c4b659098c35b3d180ed034))
- **feat(psql2):** Add \e - ([f18bdf5](https://github.com/beyondessential/bestool/commit/f18bdf588c1d002e323e2ad63bb045fbd8376d50))
- **feat(psql2):** Add \i metacommand for reading queries from file - ([ecbafb4](https://github.com/beyondessential/bestool/commit/ecbafb4df3a857bb05e3a5faa4f500da7b0c740c))
- **feat(psql2):** Add file path autocompletion for \i metacommand - ([62e34fe](https://github.com/beyondessential/bestool/commit/62e34fecaa9f79637fe2d80908eb1c8f383c5c77))
- **feat(psql2):** Improve \i autocompletion with directory listing and case-insensitive matching - ([799aeda](https://github.com/beyondessential/bestool/commit/799aeda32ee09cc9d635bbcf3e179580054c1fea))
- **feat(psql2):** Add \o metacommand parsing and file path autocompletion - ([75191ef](https://github.com/beyondessential/bestool/commit/75191ef9a7783ab3df4f2b25a2a236fe6fbb8daf))
- **feat(psql2):** Add 'o' query modifier for output to file with path autocompletion - ([609be7a](https://github.com/beyondessential/bestool/commit/609be7a94024685096f595435ff6b05df7795f64))
- **feat(psql2):** Implement \o metacommand to open/close output file - ([2d852ec](https://github.com/beyondessential/bestool/commit/2d852ec14f22c67216c61abde463e52df5661d9b))
- **feat(psql2):** Add \debug state metacommand to print ReplState - ([384be38](https://github.com/beyondessential/bestool/commit/384be382291a4f7e60c0f06a78ef9f0ebb4f0bd4))
- **feat(psql2):** Implement output file writing for \o metacommand and 'o' query modifier - ([1c56eb1](https://github.com/beyondessential/bestool/commit/1c56eb1d43b53b9fc353be1a4f96cf8ba38c9b1f))
- **feat(psql2):** Use British spelling (colours) and respect logging color setting - ([85fd38c](https://github.com/beyondessential/bestool/commit/85fd38cf2cc7a55ca561d826fa33e5a9b5c579b2))
- **feat(psql2):** Add autocompletion for \debug metacommand and state argument - ([1b23a32](https://github.com/beyondessential/bestool/commit/1b23a32b0b1a7957e4b731cc487838c9b79d0d44))
- **feat(psql2):** Show help when \debug is run without arguments - ([027245d](https://github.com/beyondessential/bestool/commit/027245ddc4069e0ade1b1a7d5f0a6770eaac8ac7))
- **feat(psql2):** Add \? and \help metacommands to list available commands - ([db5624f](https://github.com/beyondessential/bestool/commit/db5624f3159dad510373e294453925077bd8b6d0))
- **feat(psql2):** Add \set, \unset, and \vars metacommands - ([2badcdc](https://github.com/beyondessential/bestool/commit/2badcdc1d93f13d586333c4b8ff610bae8c42e31))
- **feat(psql2):** Implement \set, \unset, and \vars metacommands - ([0df6b77](https://github.com/beyondessential/bestool/commit/0df6b77891d3957f2e1821466f41a732e2a0142b))
- **feat(psql2):** Add \get metacommand to print variable value - ([9f353b1](https://github.com/beyondessential/bestool/commit/9f353b16eeb42d99777af9288c7d029269887a13))
- **feat(psql2):** Add variable name completion for \get, \set, \unset - ([72419b4](https://github.com/beyondessential/bestool/commit/72419b491aca5252c1f836a51457c726d84348b0))
- **feat(psql2):** Implement VarSet query modifier (\gset) - ([6bb9ee3](https://github.com/beyondessential/bestool/commit/6bb9ee39894d3ec0fd3e3b0404c05486b7aa1fd1))
- **feat(psql2):** Add /snip metacommand - ([e6e3035](https://github.com/beyondessential/bestool/commit/e6e3035a09e168c9f1e51aa6adc08f7866092c50))
- **feat(psql2):** Add \list[+] table [pattern] metacommand with parsing and completion - ([90c0554](https://github.com/beyondessential/bestool/commit/90c05543f2d4296157929f2a8ef021b940afe810))
- **feat(psql2):** Support * and *.* patterns to list all tables in all schemas - ([63cf90b](https://github.com/beyondessential/bestool/commit/63cf90b2311e3d345bfe7177c0c1c4b47204402d))
- **feat(psql2):** Include pg_catalog and allow explicit information_schema/pg_toast queries - ([0eb1694](https://github.com/beyondessential/bestool/commit/0eb169424b22d94927b2b6441fdbe2e68a77798f))
- **feat(psql2):** Split Access into Access method and ACL columns in detail view - ([9e471ed](https://github.com/beyondessential/bestool/commit/9e471ed7cbe5e222e7255dbe67a5f353f4cdbafb))
- **feat(psql2):** Add ! modifier to use separate connection for \list queries - ([ce017be](https://github.com/beyondessential/bestool/commit/ce017be7e2ab913579f9e199e78e1b5513639ea1))
- **feat(psql2):** Add \list index and \di commands for listing indexes - ([4e3143e](https://github.com/beyondessential/bestool/commit/4e3143e5db4ccdefc9345c6fefabdf84480769f0))
- **feat(psql2):** Display row count status in dim blue when colors enabled - ([af0960f](https://github.com/beyondessential/bestool/commit/af0960f06684cc28ee0887c6fe82eb6e00144ea3))
- **feat(psql2):** Add \list function command with \df alias - ([a585f0a](https://github.com/beyondessential/bestool/commit/a585f0a5c37ea6296b9a80bdb0dea7232f351f27))
- **feat(psql2):** Add \list view command with \dv alias - ([2da3cb7](https://github.com/beyondessential/bestool/commit/2da3cb7d8f0f80fc65107c844068d9a21c10d846))
- **feat(psql2):** Add \list schema command with \dn alias - ([9fcc03b](https://github.com/beyondessential/bestool/commit/9fcc03b3c2a817be10d5e8740bd0dc8b910ec135))
- **feat(psql2):** Add \d[+][!] <item> describe command with parsing and completion - ([8763dc0](https://github.com/beyondessential/bestool/commit/8763dc0d1e9562ae3667796ac8afcf88670a0caa))
- **feat(psql2):** Include materialized views in schema cache - ([2a4dacf](https://github.com/beyondessential/bestool/commit/2a4dacfa7f7d091285f45a168d63b6428d2df791))
- **feat(psql2):** Add \d - ([a56f468](https://github.com/beyondessential/bestool/commit/a56f468c7f8bfdc0bb08bbd53a7d2d975ace7096))
- **feat(psql2):** Tweak \d outputs - ([4858386](https://github.com/beyondessential/bestool/commit/4858386f2a0dd6ecf37694f8f029eac8108432b1))
- **feat(psql2):** Add \d for functions - ([3e78393](https://github.com/beyondessential/bestool/commit/3e78393e9e251e68e1d3edf97b605684417ee7a9))
- **feat(psql2):** Add \list sequence - ([466cf93](https://github.com/beyondessential/bestool/commit/466cf9317562976a3ba16cff0b079be4ed60e227))
- **fix(psql2):** Handle numeric and boolean types in query results - ([9a008e7](https://github.com/beyondessential/bestool/commit/9a008e713bc7dc967c5378bac76f3171fea4b231))
- **fix(psql2):** Save all queries to history and show detailed errors - ([efdcfc3](https://github.com/beyondessential/bestool/commit/efdcfc37af7493072172d2fe65cbd6b9dc0a82ba))
- **fix(psql2):** Properly reference subquery columns in text cast query - ([fd3d630](https://github.com/beyondessential/bestool/commit/fd3d630f35f04532be29a19221f6a16c1d93b9b7))
- **fix(psql2):** Strip trailing semicolon from SQL before text casting - ([cf31e73](https://github.com/beyondessential/bestool/commit/cf31e7323a03c90d7965c2bf44c3a7ea2400c10b))
- **fix(psql2):** Use correct psql prompt style (=> for regular, =# for super) - ([2439088](https://github.com/beyondessential/bestool/commit/243908838c7a39b5887098d5e450be48915417f5))
- **fix(psql2):** Save exact user input to history including \g - ([8378a55](https://github.com/beyondessential/bestool/commit/8378a55a2c89a10474e5132c7c9e5c66c87d3027))
- **fix(psql2):** Use proper tracing field syntax for modifiers debug log - ([2599b38](https://github.com/beyondessential/bestool/commit/2599b382fed1500a26c6ebc47076def69eb2c927))
- **fix(psql2):** Make Ctrl-C always clear buffer without quitting - ([e4f19c6](https://github.com/beyondessential/bestool/commit/e4f19c67f3a502eec24d06ec798d27f6127c8e0f))
- **fix(psql2):** Preserve full input including modifiers in history - ([96a8808](https://github.com/beyondessential/bestool/commit/96a880844245fbf63c5d6fd4742e5b45ddd90e0e))
- **fix(psql2):** Add missing write and ots fields to integration tests - ([c0d2ade](https://github.com/beyondessential/bestool/commit/c0d2adee11a0b3ee1f6374fb64fe28f5be9ca0cc))
- **fix(psql2):** Correct transaction marker position in prompt - ([03bf923](https://github.com/beyondessential/bestool/commit/03bf9231d78008521e2740a7dbbe694b62d878de))
- **fix(psql2):** Use separate monitor connection for transaction state detection - ([ae0c107](https://github.com/beyondessential/bestool/commit/ae0c10764efd9a1d7197cc188dc480b9ddfdd3d7))
- **fix(psql2):** Remove extra space from prompt - ([6bf404c](https://github.com/beyondessential/bestool/commit/6bf404c547120b5e83be0b45aa6308ae1574a543))
- **fix(psql2):** Reuse history database connection for OTS prompt - ([92ff554](https://github.com/beyondessential/bestool/commit/92ff554efb0437b84363acf830542d27231f5f92))
- **fix(psql2):** Only show info logs from bestool_psql2 at verbosity level 0 - ([87cfef7](https://github.com/beyondessential/bestool/commit/87cfef73e854197dd12d2069c4480ed3a61145ad))
- **fix(psql2):** Write mode must be quittable - ([43ea5cc](https://github.com/beyondessential/bestool/commit/43ea5cc9031268d5fbb1d78fb83ef063a5c4faf9))
- **fix(psql2):** Duplicate history - ([0ff8909](https://github.com/beyondessential/bestool/commit/0ff89098c77b7884f97ca2600541f8e5676f053a))
- **fix(psql2):** Colour bleed with json output - ([e7536a2](https://github.com/beyondessential/bestool/commit/e7536a26fcba7dff04566079be4ab668b09467dd))
- **fix(psql2):** Resolve clippy warnings - ([b330388](https://github.com/beyondessential/bestool/commit/b3303887117ac4ab5fe6893aa5ef595bcd7ac8d6))
- **fix(psql2):** Block SIGINT during query execution to prevent rustyline crash - ([afef970](https://github.com/beyondessential/bestool/commit/afef9707bda7626c95b97164442298d270c97960))
- **fix(psql2):** Use global SIGINT handler to prevent rustyline crash - ([5fe1cba](https://github.com/beyondessential/bestool/commit/5fe1cba00c79b757ca4138d6b913be8ab7c973b1))
- **fix(psql2):** Add metacommands like \W to history/audit log - ([b8a38d3](https://github.com/beyondessential/bestool/commit/b8a38d37fe9de431a9f1eb23029548841bce7462))
- **fix(psql2):** Refuse to disable write mode unless transaction is idle or none - ([4af0bf0](https://github.com/beyondessential/bestool/commit/4af0bf0def76a9778637ab9832c13c61c2dda457))
- **fix(psql2):** Add all metacommands to history (including \e and \i) - ([522bae4](https://github.com/beyondessential/bestool/commit/522bae4ee509dd5052148913c63e7215a7f386d9))
- **fix(psql2):** Parser for \snip - ([f48cd07](https://github.com/beyondessential/bestool/commit/f48cd07be2e163bf44ae2e1fb000665ae9c88455))
- **fix(psql2):** Remove redundant 'static lifetimes from constants - ([3ca874f](https://github.com/beyondessential/bestool/commit/3ca874fe9d91f0090b741e3e961700183b8c807f))
- **fix(psql2):** \d table output - ([d230c86](https://github.com/beyondessential/bestool/commit/d230c860f53e95953b6d2d7af0edeac36381592c))
- **refactor(psql2):** Change to positional dbname argument - ([697931f](https://github.com/beyondessential/bestool/commit/697931f4c9a7f9aae198e7c1351557d034fb856a))
- **refactor(psql2):** Use compact JSON formatting in result rows - ([ace7ff3](https://github.com/beyondessential/bestool/commit/ace7ff3dd8b2a50d02c064d4a71bc958a64399cd))
- **refactor(psql2):** Use winnow for case-insensitive metacommand parsing - ([0d54104](https://github.com/beyondessential/bestool/commit/0d54104538e9428ffc5e5477c14b2a49a2ac7bfd))
- **refactor(psql2):** Split lib.rs into separate modules - ([64fba03](https://github.com/beyondessential/bestool/commit/64fba038703687e502b0f85dc4659c2d07b7feb3))
- **refactor(psql2):** Restrict public API with pub(crate) - ([a4ef1bc](https://github.com/beyondessential/bestool/commit/a4ef1bc0202660bc2fd7415c99a9d68c5fe0f9cc))
- **refactor(psql2):** Change QueryModifiers to HashSet of enum variants - ([1dfaa81](https://github.com/beyondessential/bestool/commit/1dfaa81b5f92e61010ad949f7aadaab64f1ded15))
- **refactor(psql2):** Change parse_query_modifiers to return Result<Option<_>> - ([39476cf](https://github.com/beyondessential/bestool/commit/39476cfa750cd74f69bdcaab9c43fc55cf5d966a))
- **refactor(psql2):** Parse query only once and save quit commands to history - ([2e0cccb](https://github.com/beyondessential/bestool/commit/2e0cccbeb50775db8ecb385b92e1d0a9dbe4a793))
- **refactor(psql2):** Change write mode toggle logs to debug level - ([e6f0fff](https://github.com/beyondessential/bestool/commit/e6f0fffa14ad3319096daadcbfe86cef9ca781aa))
- **refactor(psql2):** Extract REPL functions and use ControlFlow - ([8a4de35](https://github.com/beyondessential/bestool/commit/8a4de35f1bbcbd7e462cee5645e39f0771f48209))
- **refactor(psql2):** Extract ReplAction handlers into separate methods - ([49ab0b8](https://github.com/beyondessential/bestool/commit/49ab0b8c4ec856c180aa6861031ca925493f5059))
- **refactor(psql2):** Minor to avoid keeping rewriting the match - ([10c4b3d](https://github.com/beyondessential/bestool/commit/10c4b3dea5556493fdfc7c832c07c4c4d92b124e))
- **refactor(psql2):** General cleanups, and fix state locking bug - ([2716194](https://github.com/beyondessential/bestool/commit/2716194ed302626e4809afcc3b1c17dc7a76d750))
- **refactor(psql2):** Split handle_input() into input.rs module - ([c81361f](https://github.com/beyondessential/bestool/commit/c81361f4ab3769e96c2e6fd15c105db469b2f052))
- **refactor(psql2):** Merge run() into repl module and compact history on exit - ([f7a963c](https://github.com/beyondessential/bestool/commit/f7a963caa4f41c64f096176b0fef72b7420da856))
- **refactor(psql2):** Rename History to Audit throughout codebase - ([3f1927c](https://github.com/beyondessential/bestool/commit/3f1927c2cae14d8da001987e02a51cd7c8fc0dd4))
- **refactor(psql2):** Use a connection pool - ([9d8e29b](https://github.com/beyondessential/bestool/commit/9d8e29bc550394b3a671822aee25cb3e2316be1a))
- **refactor(psql2):** Use crossterm (already in tree) to get term size - ([b3e9f72](https://github.com/beyondessential/bestool/commit/b3e9f7236e5d8a4274f4e9991f45291cc40d6c9f))
- **refactor(psql2):** Use rustyline's signal-hook feature for better signal integration - ([1f2a610](https://github.com/beyondessential/bestool/commit/1f2a61016afb5f1a689b418b68a8e040056702f8))
- **refactor(psql2):** Extract table styling config to shared helper - ([9223c2f](https://github.com/beyondessential/bestool/commit/9223c2fce2d835004e52deb0adc7d8d0ca82c5cb))
- **refactor(psql2):** Simplify execute_query with context structs and fix clippy - ([603ed58](https://github.com/beyondessential/bestool/commit/603ed5817da7907a9f8f3db10a9106c59e7be42a))
- **refactor(psql2):** Simplify compaction - ([2f592e5](https://github.com/beyondessential/bestool/commit/2f592e5ceb1fc29c0e63eec454527d7cfbdeac7a))
- **refactor(psql2):** Use Path appropriately - ([db18dcf](https://github.com/beyondessential/bestool/commit/db18dcf1e78d2f9c2e92871b29bcfa3fdff65d6e))
- **refactor(psql2):** Restore ReplState::new() to being test-only - ([3f28f9a](https://github.com/beyondessential/bestool/commit/3f28f9a4664ee2585be9b24c4fe7b19719e8fb0d))
- **refactor(psql2):** *actually* merge imports - ([fbb35e4](https://github.com/beyondessential/bestool/commit/fbb35e49e2d5d11acf055fa793ac167d0cf79cf7))
- **refactor(psql2):** Actually break up repl.rs - ([46bf4ef](https://github.com/beyondessential/bestool/commit/46bf4ef7a1a5f93055afb08161716666492f6563))
- **refactor(psql2):** Break up audit.rs - ([ed0955a](https://github.com/beyondessential/bestool/commit/ed0955a92efe0a4b1dfee08a4f0932194ce3a298))
- **refactor(psql2):** Break up completer.rs - ([2abd6e4](https://github.com/beyondessential/bestool/commit/2abd6e42d46ef4c1c5085114c61b3bfa118e155c))
- **refactor(psql2):** Rename highlighter to what it actually contains, the - ([130e7f4](https://github.com/beyondessential/bestool/commit/130e7f47472b0ca43a91dd333312835469c20d31))
- **refactor(psql2):** Move signal handling to leave lib.rs as a barrel file - ([e5d29f9](https://github.com/beyondessential/bestool/commit/e5d29f9188c8a4f596d543fe412b133501d4191f))
- **refactor(psql2):** Rename PsqlConfig to just Config - ([90144af](https://github.com/beyondessential/bestool/commit/90144aff86e9665574923018a8d92263a7003cfc))
- **refactor(psql2):** Break up parser - ([8314a1f](https://github.com/beyondessential/bestool/commit/8314a1f1d3eaa980aebdc6ced98accc042af5a82))
- **refactor(psql2):** Break up query - ([4985ea3](https://github.com/beyondessential/bestool/commit/4985ea3b24ebb52af4a71a04caddc2849e48d439))
- **refactor(psql2):** Adjust columns for table and index listing - ([068a1b7](https://github.com/beyondessential/bestool/commit/068a1b793bb3ea573d65dc48c6de9aa1dcca3e32))
- **refactor(psql2):** Split list handler into submodules - ([9f8aa1e](https://github.com/beyondessential/bestool/commit/9f8aa1e70335fc8d117442d486de81c233dafb1b))
- **refactor(psql2):** Improve \list view output - ([d1942dc](https://github.com/beyondessential/bestool/commit/d1942dcafa6bc591adf4862f8434b4adbfaeb2d0))
- **refactor(psql2):** Remove Description column from \list schema - ([d774fa1](https://github.com/beyondessential/bestool/commit/d774fa152ec77387030f2508d239ef3c9f8933d7))
- **test(psql2):** Add unit and integration tests - ([dedd378](https://github.com/beyondessential/bestool/commit/dedd378724a7853aa15acbdca7ff516ce136f29b))
- **test(psql2):** Add tests for text casting and type formatting - ([1430eec](https://github.com/beyondessential/bestool/commit/1430eec34816ea0c7adb7b6f5a1318705083ccad))
- **test(psql2):** Remove useless placeholder test - ([57ec470](https://github.com/beyondessential/bestool/commit/57ec4707747054bb6ce79189bab29181790f5bb8))
- **test(psql2):** Add tests for REPL input handling and control flow - ([c494bcd](https://github.com/beyondessential/bestool/commit/c494bcd7ee3a8fceea3f0f1c0f1f1d76df69f9cf))
- **test(psql2):** Remove useless test - ([5aa6898](https://github.com/beyondessential/bestool/commit/5aa689851aafe6221e359b29b09f55063d05f436))
- **test(psql2):** Fix integration test - ([8dd820a](https://github.com/beyondessential/bestool/commit/8dd820ac49b64c6100829b3e1b5b4101f5b3a75b))
- **test(psql2):** Add database integration tests for \list and \dt commands - ([767c2fe](https://github.com/beyondessential/bestool/commit/767c2fe4b986897c7ebdfa034f0e102b252211e2))
- **test(psql2):** Remove #[ignore] from integration tests - ([85431c1](https://github.com/beyondessential/bestool/commit/85431c17948cdbc2ffa7617cd9ec37bdcda09b61))
- **test(psql2):** Add crash tests for \d - ([e3c6723](https://github.com/beyondessential/bestool/commit/e3c6723a7b1748e64a3f9d21837f53f864f8f8ac))
- **test(psql2):** Add output tests for \d - ([e693cf6](https://github.com/beyondessential/bestool/commit/e693cf6e644417713511ff70f810abe6ce63b6e3))
- **tweak(psql2):** Adjust table styles for readability - ([5175323](https://github.com/beyondessential/bestool/commit/51753239b89c8dd6e28044351fb0fd0f47367593))
- **tweak(psql2):** \d view output - ([86f9009](https://github.com/beyondessential/bestool/commit/86f90093aa4f69722e09352b4353b566f27df6d8))
- **tweak(psql2):** Slightly better error messages for \d - ([929018b](https://github.com/beyondessential/bestool/commit/929018b315c9140bbdca0981a3ddd68fd0da6b36))
- **tweak(psql2):** More error handling - ([79a7e4a](https://github.com/beyondessential/bestool/commit/79a7e4a7418a7cddbcac6190c83e81b79952201d))
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




- **fix(psql):** Reader thread issues on windows - ([75ba94b](https://github.com/beyondessential/bestool/commit/75ba94b790c58cb29b5afc0df4a78e304b23f27d))
- **fix(psql):** Check the correct buffer for the prompt - ([c6d490e](https://github.com/beyondessential/bestool/commit/c6d490ed373def330c24066764564a5a048e7025))
- **fix(psql):** Check the correct buffer for the prompt - ([c6d490e](https://github.com/beyondessential/bestool/commit/c6d490ed373def330c24066764564a5a048e7025))
- **fix(psql):** Wait more intelligently for files to be written in schema - ([32ca85e](https://github.com/beyondessential/bestool/commit/32ca85e3606c8b74d3f79a745a6d9ad8b2921130))
- **fix(psql):** Windows paths are backslashed, but psql doesn't like that - ([bd2caa5](https://github.com/beyondessential/bestool/commit/bd2caa5692a02a8408b7ca34eab50be42c1070fb))
- **refactor(psql):** Clean up legacy field name - ([72f125a](https://github.com/beyondessential/bestool/commit/72f125a4eb9e0c5ea263660769b3185f94670290))
- **tweak(psql):** Sort root tables before additional ones - ([145349f](https://github.com/beyondessential/bestool/commit/145349f0a736dcab556f3a4350f0fb15df431e5b))
---
## [1.0.8](https://github.com/beyondessential/bestool/compare/v1.0.7..v1.0.8) - 2025-10-21




- **fix(psql):** Check process status during read loop to avoid blocking on exit - ([27565de](https://github.com/beyondessential/bestool/commit/27565de9f38f2a215b288db46b7b079800d8be8f))
---
## [1.0.7](https://github.com/beyondessential/bestool/compare/v1.0.6..v1.0.7) - 2025-10-21




- **fix(psql):** Use CRLF on windows - ([780b519](https://github.com/beyondessential/bestool/commit/780b5194525d723320ab152e1049582c27597a9c))
---
## [1.0.6](https://github.com/beyondessential/bestool/compare/v1.0.5..v1.0.6) - 2025-10-21




- **fix(psql):** More windows-only behaviours - ([fb3e38b](https://github.com/beyondessential/bestool/commit/fb3e38bdbb919f597da896981e68b10287c9cced))
---
## [1.0.5](https://github.com/beyondessential/bestool/compare/v1.0.4..v1.0.5) - 2025-10-21




- **fix(psql):** Disable pager on windows - ([03ce48c](https://github.com/beyondessential/bestool/commit/03ce48c2f38c8d5e64eaa061da97d29df6dc2d3e))
- **fix(psql):** Warn when not running in powershell on windows - ([e3ff56a](https://github.com/beyondessential/bestool/commit/e3ff56adff97dc4b142210c46dfca36652f6f4b6))
---
## [1.0.4](https://github.com/beyondessential/bestool/compare/v1.0.3..v1.0.4) - 2025-10-21




- **fix(psql):** Disable schema autocompletion by default - ([85d1452](https://github.com/beyondessential/bestool/commit/85d1452093cc3d6f2c9193cb739389574f0723c0))
---
## [1.0.3](https://github.com/beyondessential/bestool/compare/v1.0.2..v1.0.3) - 2025-10-21




- **fix(psql):** Windows ptys are cursed - ([d373fbb](https://github.com/beyondessential/bestool/commit/d373fbbd51b620f1f06266678676ecee84a8ca98))
---
## [1.0.2](https://github.com/beyondessential/bestool/compare/v1.0.1..v1.0.2) - 2025-10-21




- **fix(psql):** Find psql program when not in PATH - ([e1004bb](https://github.com/beyondessential/bestool/commit/e1004bb75f55c3316eb106331b756b7659db1fdd))
---
## [1.0.1](https://github.com/beyondessential/bestool/compare/v1.0.0..v1.0.1) - 2025-10-20




- **fix(psql):** Don't copy env - ([48617ca](https://github.com/beyondessential/bestool/commit/48617ca3dce0218fd8d75b03a57c0f0c2b630133))
---
## [1.0.0](https://github.com/beyondessential/bestool/compare/v0.30.3..v1.0.0) - 2025-10-20


- **deps:** Update deps - ([062969e](https://github.com/beyondessential/bestool/commit/062969e2dee5b5dece0d0c4c0769450cdc6601b0))
- **deps:** Upgrade all deps (#21) - ([ff2ac52](https://github.com/beyondessential/bestool/commit/ff2ac52790aea2b1744b7bff589a24bc7929b259))
- **deps:** Explicit tracing-attributes to get aarch64-gnu to build - ([8fd1c33](https://github.com/beyondessential/bestool/commit/8fd1c338e9d490f9ebf44fbf3d3349e4e6280415))
- **deps:** Bump tokio from 1.36.0 to 1.37.0 (#31) - ([935d77b](https://github.com/beyondessential/bestool/commit/935d77b02bd3ff81cf87d8305e664a78492fbe6b))
- **deps:** Bump serde_json from 1.0.114 to 1.0.115 (#30) - ([e0a38fa](https://github.com/beyondessential/bestool/commit/e0a38fadc15af75cebc8109031d972108ba39869))
- **deps:** Bump aws-config from 1.1.8 to 1.1.9 (#29) - ([b04f866](https://github.com/beyondessential/bestool/commit/b04f86659a1cbbe4a3ad2aa3f8c09f0088151d10))
- **deps:** Bump regex from 1.10.3 to 1.10.4 (#28) - ([523cbcc](https://github.com/beyondessential/bestool/commit/523cbcc515cf16a235ffc5f118e7c3421c45177a))
- **deps:** Bump bytes from 1.5.0 to 1.6.0 (#27) - ([db592a5](https://github.com/beyondessential/bestool/commit/db592a56d985a6c28bb8bf260b49785269a1ce76))
- **deps:** Bump clap from 4.5.3 to 4.5.4 (#34) - ([dbbeaca](https://github.com/beyondessential/bestool/commit/dbbeaca90af8fcb08ff5b83505c39d097bad287e))
- **deps:** Bump aws-sdk-route53 from 1.18.0 to 1.19.0 (#37) - ([40bbaa9](https://github.com/beyondessential/bestool/commit/40bbaa9b550f55fdf7692424c9ad893412cf809c))
- **deps:** Bump aws-sdk-s3 from 1.20.0 to 1.21.0 (#35) - ([156d439](https://github.com/beyondessential/bestool/commit/156d439be5d87d6eb752f9ba4d820781c4aa6dc5))
- **deps:** Bump chrono from 0.4.35 to 0.4.37 (#36) - ([abe847a](https://github.com/beyondessential/bestool/commit/abe847a46ba076b9de4c0fec8bae191bb18cc27a))
- **deps:** Bump h2 from 0.3.25 to 0.3.26 (#41) - ([b7686eb](https://github.com/beyondessential/bestool/commit/b7686eb7f158decdf7822177d200ab7447499c9f))
- **deps:** Bump aws-sdk-s3 from 1.22.0 to 1.23.0 (#42) - ([9285ddc](https://github.com/beyondessential/bestool/commit/9285ddcc9e270ecd390cc5e45f7ae71f7d44feaf))
- **deps:** Bump windows from 0.54.0 to 0.56.0 (#43) - ([2b1783c](https://github.com/beyondessential/bestool/commit/2b1783cef5557719434206045d88f088462fb8e9))
- **deps:** Bump reqwest from 0.11.27 to 0.12.3 (#45) - ([2071ec7](https://github.com/beyondessential/bestool/commit/2071ec76b7d73f4f117f69766c8bf670737bdbfa))
- **deps:** Bump aws-config from 1.1.10 to 1.2.0 (#44) - ([72afd30](https://github.com/beyondessential/bestool/commit/72afd301942bd6023c0d8ff54cdf4b1aa4738fe8))
- **deps:** Bump rustls from 0.21.10 to 0.21.11 (#47) - ([8c0dd30](https://github.com/beyondessential/bestool/commit/8c0dd305c13fefdf03645d6314af13933499f543))
- **deps:** Bump build-data from 0.1.5 to 0.2.1 (#49) - ([123ec53](https://github.com/beyondessential/bestool/commit/123ec5334e1a233252c1f8dcc163ecdda269a04f))
- **deps:** Bump aws-sdk-route53 from 1.20.0 to 1.21.0 (#48) - ([28b14e2](https://github.com/beyondessential/bestool/commit/28b14e21ec16d6bf2df9f3507a075e56101605b4))
- **deps:** Bump serde from 1.0.197 to 1.0.198 (#51) - ([3a59358](https://github.com/beyondessential/bestool/commit/3a593587edafe21e78b49aaac88311f6e138d82a))
- **deps:** Bump thiserror from 1.0.58 to 1.0.59 (#52) - ([26f226f](https://github.com/beyondessential/bestool/commit/26f226fa71deab66653cfbdd6ea207f4bd668413))
- **deps:** Bump chrono from 0.4.37 to 0.4.38 (#50) - ([5340a0e](https://github.com/beyondessential/bestool/commit/5340a0e9e60874508dc9a7627bf17191102feb72))
- **deps:** Bump aws-sdk-route53 from 1.21.0 to 1.22.0 (#53) - ([b368579](https://github.com/beyondessential/bestool/commit/b368579263549ae97da0c6531582d14a6c4efc17))
- **deps:** Bump serde_json from 1.0.115 to 1.0.116 (#55) - ([56a4682](https://github.com/beyondessential/bestool/commit/56a4682aa5707d31b72d0efca113018c1d239ce4))
- **deps:** Bump serde from 1.0.198 to 1.0.199 (#56) - ([0d6114e](https://github.com/beyondessential/bestool/commit/0d6114ed401ac5c0b35cfacf97645656f41f0d99))
- **deps:** Bump ssh-key from 0.6.5 to 0.6.6 (#57) - ([4fca950](https://github.com/beyondessential/bestool/commit/4fca9507a95aca694d6f7e84244d9dc55d8b0855))
- **deps:** Bump upgrade from 1.1.1 to 2.0.0 (#54) - ([2223817](https://github.com/beyondessential/bestool/commit/222381743fadf142e3bcc67c7d57624e12b194a8))
- **deps:** Bump serde from 1.0.199 to 1.0.200 (#58) - ([f35c68d](https://github.com/beyondessential/bestool/commit/f35c68dfdfabc85e3cd2fbf700f01dd6c2b91dc0))
- **deps:** Bump mimalloc from 0.1.39 to 0.1.41 (#59) - ([0240302](https://github.com/beyondessential/bestool/commit/0240302b9adf60ab437095145c042c7242d5dc2c))
- **deps:** Bump aws-sdk-route53 from 1.22.0 to 1.23.0 (#62) - ([9320982](https://github.com/beyondessential/bestool/commit/932098212c996a3f707ad511d729ea7a01c06ea7))
- **deps:** Bump reqwest from 0.12.3 to 0.12.4 (#61) - ([aaf0364](https://github.com/beyondessential/bestool/commit/aaf0364f6dda58719a7d78f95591226a8ad653a3))
- **deps:** Bump aws-sdk-sts from 1.20.0 to 1.22.0 (#63) - ([b078c77](https://github.com/beyondessential/bestool/commit/b078c7786d46de9d76b0b29f7c6693bc660396d8))
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
- **deps:** Update rpi-st7789v2-driver deps - ([cab2c8b](https://github.com/beyondessential/bestool/commit/cab2c8b6ab1a9d79ab0612924ba13cb8180296a4))
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
- **deps:** Update lockfile - ([79408b2](https://github.com/beyondessential/bestool/commit/79408b27fbd5f4ee4e917fd91d7b55924a8b6dc2))
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
- **deps:** Update lockfile - ([cb2fe0d](https://github.com/beyondessential/bestool/commit/cb2fe0d4aab882afcc0460b6181a56cd795bcf49))
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
- **deps:** Upgrade rand to 0.9.0 - ([ec15f23](https://github.com/beyondessential/bestool/commit/ec15f2356f46691ee4b0d8929d8038a634a24f43))
- **deps:** Make html2md conditional - ([a0af4fa](https://github.com/beyondessential/bestool/commit/a0af4fa054a519f1c70e47773516628acb4a379c))
- **deps:** Update lockfile - ([4c4b0b1](https://github.com/beyondessential/bestool/commit/4c4b0b154b826e5664295601d01c2c079dfd5119))
- **deps:** Bump the deps group across 1 directory with 9 updates (#186) - ([706ea9d](https://github.com/beyondessential/bestool/commit/706ea9dc8e6958e40b24e7a5efff18d1f1fd9896))
- **deps:** Update lockfile - ([2b56968](https://github.com/beyondessential/bestool/commit/2b56968a328648316523487878ed4f5df781969f))
- **deps:** Bump the deps group across 1 directory with 14 updates (#188) - ([650984b](https://github.com/beyondessential/bestool/commit/650984b23fb7bc6bb12b722489f10588d2747c62))
- **deps:** Bump the deps group across 1 directory with 15 updates (#193) - ([751581d](https://github.com/beyondessential/bestool/commit/751581df704570d3eaa57ef1a64f8da152fb2d79))
- **deps:** Update lockfile - ([8a975a9](https://github.com/beyondessential/bestool/commit/8a975a94bcf432094dfc7a54cbf9c107b6e676f7))
- **deps:** Bump the deps group with 7 updates (#196) - ([bbdf6c0](https://github.com/beyondessential/bestool/commit/bbdf6c0cfcaa489684aefca3757b7fcb568eb668))
- **deps:** Bump the deps group with 11 updates (#197) - ([2d6e431](https://github.com/beyondessential/bestool/commit/2d6e431759cf24a492881cc3ce0d62bc1c473105))
- **deps:** Bump the deps group across 1 directory with 32 updates (#217) - ([d8693fc](https://github.com/beyondessential/bestool/commit/d8693fc00b4cb98a269488f0d3c4cf4a89646015))
- **deps:** Bump the deps group with 8 updates (#218) - ([d4003b2](https://github.com/beyondessential/bestool/commit/d4003b2b9fcbeaf73ce6994c1f146c56c8da154b))
- **deps:** Bump the deps group with 6 updates (#220) - ([93e8e9f](https://github.com/beyondessential/bestool/commit/93e8e9f86ff28f0d158f3891d850ab01eba7dc90))
- **deps:** Bump the deps group with 6 updates (#222) - ([17c8340](https://github.com/beyondessential/bestool/commit/17c83404bec0be56929530e6e6457d653e2e74a9))
- **deps:** Update transitives - ([ff4d551](https://github.com/beyondessential/bestool/commit/ff4d55198390da63f21149d756e67cf14cc58aea))
- **deps:** Update transitives - ([fb2be95](https://github.com/beyondessential/bestool/commit/fb2be95cbb4e732760954fc1a0a0a50822718cc6))
- **deps:** Upgrade psql deps - ([2575068](https://github.com/beyondessential/bestool/commit/2575068d9da8e8bd585d4fb6b5d2c4eb9079111c))
- **deps:** Fix optional deps - ([d1c79ea](https://github.com/beyondessential/bestool/commit/d1c79ea929bb6140c8a73856a400263f9a9b3213))
- **doc:** Add downloads for the current version - ([fc7a147](https://github.com/beyondessential/bestool/commit/fc7a1475e5f00cac888278a74348a0f664eded7c))
- **doc:** Provide links to latest URLs - ([25a7ab7](https://github.com/beyondessential/bestool/commit/25a7ab75fe4bd638c91e844c32ec9f0b6f06d765))
- **doc:** Add development guide - ([95f31bc](https://github.com/beyondessential/bestool/commit/95f31bcdd49c2d7b2aef3c54d08f8ee6e4c0d76e))
- **doc:** Show how to use bestool in GHA - ([b7f154f](https://github.com/beyondessential/bestool/commit/b7f154ff9577f78417584570ee58d04c2cc2da4d))
- **doc:** Add contributing.md and code of conduct - ([63b1505](https://github.com/beyondessential/bestool/commit/63b15053cd6e630131868333e545b4c0b4689549))
- **doc:** Mention self-update in readme - ([301a3a1](https://github.com/beyondessential/bestool/commit/301a3a12ce49a906f9226a71d594ec5af57e4fcf))
- **doc:** Remove obsolete link - ([1f5fc7f](https://github.com/beyondessential/bestool/commit/1f5fc7ffa82552f804a20ffba3915734ac8134ee))
- **doc:** Add flags/commands to docsrs output - ([deca19c](https://github.com/beyondessential/bestool/commit/deca19c34d41271a273eb85d728cdab565a6a2d3))
- **doc:** Add docs.rs-only annotations for ease of use - ([ee9750b](https://github.com/beyondessential/bestool/commit/ee9750b351c7235fe84c02fc506f0910560215f6))
- **doc:** Fix paragraph - ([bbb125f](https://github.com/beyondessential/bestool/commit/bbb125feee581935b20201e30cf9859e84920774))
- **doc:** Remove useless ?latest from readme - ([c02c91b](https://github.com/beyondessential/bestool/commit/c02c91b415263a17bcdd6872c668faeedaa71a3d))
- **doc:** Fix changelog - ([3d83c7b](https://github.com/beyondessential/bestool/commit/3d83c7b3e158efd3c864b04750a40683e7a752e9))
- **doc:** Document aliases - ([4308848](https://github.com/beyondessential/bestool/commit/430884834f42a078bf200bb49a3d8672c23ccb78))
- **feat:** Add progress bars - ([a660095](https://github.com/beyondessential/bestool/commit/a660095cb74b79b61407f96159993953bff9f135))
- **feat:** Make it possible to turn commands off at compile time - ([904097a](https://github.com/beyondessential/bestool/commit/904097a308a075fb9c88bba9817b8e4822320f3f))
- **feat:** Support NO_COLOR (https://no-color.org) - ([b5f95ac](https://github.com/beyondessential/bestool/commit/b5f95acfd9da16e3e6e48eab3e9e02c050e61a10))
- **feat:** Add self-update command - ([64ad496](https://github.com/beyondessential/bestool/commit/64ad49603d573705825be7c1bfbe35dcb16509a4))
- **feat:** Add caddy command - ([ed275e9](https://github.com/beyondessential/bestool/commit/ed275e99d782d35ea285d04a5decc59fbfb2dca9))
- **feat:** Print info logs by default - ([c908388](https://github.com/beyondessential/bestool/commit/c90838861fcb9f253981058c4b53dc7e4d8fa0e4))
- **feat:** Control log colour usage with --color - ([6aae619](https://github.com/beyondessential/bestool/commit/6aae619aed89bd61ecd364837563bfd38e52577f))
- **feat:** Enable ansi colours on windows - ([f0f9632](https://github.com/beyondessential/bestool/commit/f0f9632e4ac0ccbea69862ef1d0e2a2e6ee93abc))
- **feat:** Enable installing with Binstall - ([45b7a74](https://github.com/beyondessential/bestool/commit/45b7a741d7c9581ab148bed5820dd5feef883c31))
- **feat:** Add --logs-timeless - ([56e3b0c](https://github.com/beyondessential/bestool/commit/56e3b0c636575b784cac3021e2cd455435d21b8f))
- **feat:** KAM-296: Backup Configs (#166) - ([fcf94bb](https://github.com/beyondessential/bestool/commit/fcf94bbe9b30c6a85e766e4170f0acf6797bd8c7))
- **feat:** KAM-341: split and join files (with backup support) (#194) - ([ea3e9f9](https://github.com/beyondessential/bestool/commit/ea3e9f9737f1db5460e8666a890e44c212524bc0))
- **feat:** Add psql wrapper - ([d182c52](https://github.com/beyondessential/bestool/commit/d182c5255eb5be8b23b9efc98d82e589ea338706))
- **fix:** Clap test - ([1d5f72c](https://github.com/beyondessential/bestool/commit/1d5f72c7761a06c6c841ca9b9707a986d00acd6b))
- **fix:** Run pm2 with cmd - ([3ef4ccc](https://github.com/beyondessential/bestool/commit/3ef4cccb1d1404100f8c23082ceeec4c1ed6d399))
- **fix:** Run yarn with cmd - ([8575b40](https://github.com/beyondessential/bestool/commit/8575b4025ba1e40a78b4b9f17fbe10ea42d3077d))
- **fix:** Fix ci - ([01cd6b4](https://github.com/beyondessential/bestool/commit/01cd6b47ba32b03f71f9e7940697352e3e52da7a))
- **fix:** Don´t require git in docsrs - ([66b5345](https://github.com/beyondessential/bestool/commit/66b5345536e7a69b5795bb00139a1c93d7915194))
- **fix:** Ability to build with cargo install - ([4a2d724](https://github.com/beyondessential/bestool/commit/4a2d72432cd8666caea418c1393e670d9e8fc2d6))
- **fix:** Just remove the git build info - ([429fefb](https://github.com/beyondessential/bestool/commit/429fefba54ecd99f13058ceb819297cd246e356e))
- **fix:** Fix tests - ([7a383e5](https://github.com/beyondessential/bestool/commit/7a383e5132d506264d7f06d1332e427034affe7f))
- **fix:** Fix tests - ([e94131c](https://github.com/beyondessential/bestool/commit/e94131cc54096669b7692f68253ebba6b8e1ad49))
- **fix:** Fix more tests - ([65951f8](https://github.com/beyondessential/bestool/commit/65951f8ff0b4435051094dabda4ad67a2347ccfe))
- **fix:** Whoops extraneous `async` - ([96ccc26](https://github.com/beyondessential/bestool/commit/96ccc2683a2da3914044a0af0b44aee6bedbfe6b))
- **fix:** Whoops windows things again - ([c632a00](https://github.com/beyondessential/bestool/commit/c632a00457805a1e54233de75608ecf24ed48eae))
- **fix:** Fix psqlrc use - ([fd55122](https://github.com/beyondessential/bestool/commit/fd55122529c79f2f491c6bf16c2a9c6b0fbedc46))
- **fix:** Fix - ([e1ab595](https://github.com/beyondessential/bestool/commit/e1ab595773d438ac338fe9ed3876d7176dcad25a))
- **fix:** Fix autocommit setting - ([e1307ed](https://github.com/beyondessential/bestool/commit/e1307ed0da6162157b33f1f25f5b450db229b7d2))
- **fix:** Fix db handle lock problem - ([5208e35](https://github.com/beyondessential/bestool/commit/5208e3597c285cbf10a86d9a56cb9cf04f83b5d1))
- **fix:** Codepage setting - ([44f0317](https://github.com/beyondessential/bestool/commit/44f03172da24129b82dc5151b7776cd7206ba7ce))
- **fix:** Default-run - ([eead678](https://github.com/beyondessential/bestool/commit/eead6781887e25a493439ab2b3ea8c97f59c67d4))
- **refactor:** Use non-blocking logging - ([56e3b0c](https://github.com/beyondessential/bestool/commit/56e3b0c636575b784cac3021e2cd455435d21b8f))
- **refactor:** Move bestool crate to a workspace (#65) - ([69cc88c](https://github.com/beyondessential/bestool/commit/69cc88c9ea65b4c8ac6af03177c1597a26cb747b))
- **refactor:** Split out rpi-st7789v2-driver crate - ([69cc88c](https://github.com/beyondessential/bestool/commit/69cc88c9ea65b4c8ac6af03177c1597a26cb747b))
- **refactor:** Remove upload command - ([eeebc93](https://github.com/beyondessential/bestool/commit/eeebc93c44e6aff8a1dd081a9ad5bc07fb3669ec))
- **refactor:** Fix missing-feature warnings - ([dca1480](https://github.com/beyondessential/bestool/commit/dca14809ac5affabbc262aac43b3bd273cf5fbfe))
- **refactor:** Remove console-subscriber feature - ([0ff3ff7](https://github.com/beyondessential/bestool/commit/0ff3ff7831d1c294b33cfcc81bcd68d18e2ab860))
- **refactor:** Deduplicate subcommands! macro - ([9e077c4](https://github.com/beyondessential/bestool/commit/9e077c43c17dcaf97db1d4acb2cae1042cb78d24))
- **refactor:** Allow mulitple #[meta] blocks in subcommands! - ([9c81a84](https://github.com/beyondessential/bestool/commit/9c81a8400f6c45ba790342f07c2403aa72552df7))
- **refactor:** Use lloggs instead of custom logging code - ([0297fdc](https://github.com/beyondessential/bestool/commit/0297fdc3bb30584e3cb28effdf3645f8a3b5197a))
- **repo:** Add editorconfig - ([b6701d0](https://github.com/beyondessential/bestool/commit/b6701d0dbf6c8a35f6b5c3591255dec98e0050fa))
- **repo:** Don't publish this - ([531c90f](https://github.com/beyondessential/bestool/commit/531c90fa0efc4a10d9ff1b903c32515943b42335))
- **repo:** Ignore tokens - ([79072e9](https://github.com/beyondessential/bestool/commit/79072e97954fb6b0d7f2f5c943dc0eb7311975ac))
- **repo:** Add release.toml for cargo-release - ([0d2748b](https://github.com/beyondessential/bestool/commit/0d2748ba92ce38db0cdc1d882a323e69fd1b5265))
- **repo:** Try harder to avoid that "chore" type - ([395b6c8](https://github.com/beyondessential/bestool/commit/395b6c8839ce501e1bd6f8b4fec3e596320c28ac))
- **repo:** Open source with GPLv3! - ([e808e5b](https://github.com/beyondessential/bestool/commit/e808e5b6b5d51464dc2170d534eb65696e24d5f1))
- **repo:** Add `tweak` conventional prefix - ([63b1505](https://github.com/beyondessential/bestool/commit/63b15053cd6e630131868333e545b4c0b4689549))
- **repo:** Add `wip` conventional prefix - ([63b1505](https://github.com/beyondessential/bestool/commit/63b15053cd6e630131868333e545b4c0b4689549))
- **repo:** Enable publishing - ([9060e6e](https://github.com/beyondessential/bestool/commit/9060e6efb1852c2e4ebe6ac910904ed98538a251))
- **repo:** Fix parsing conventional commit types - ([65d7405](https://github.com/beyondessential/bestool/commit/65d740585fb7a441736ccb51d4cfc274c74304e0))
- **repo:** Normalise change line casing - ([e9305b6](https://github.com/beyondessential/bestool/commit/e9305b6ce2b70dd8b15b6c4742629c625302baa1))
- **repo:** Remove broken aarch64-gnu build - ([49b8f3e](https://github.com/beyondessential/bestool/commit/49b8f3e5ed8156e03df7583e2e05cf0001748a9b))
- **repo:** Temporarily downgrade algae to 0.0.0 for release purposes - ([9d564c6](https://github.com/beyondessential/bestool/commit/9d564c6670af75f952c86733b908e8fd6ac3266a))
- **repo:** Temporarily disable publishing to crates.io - ([8e8dd29](https://github.com/beyondessential/bestool/commit/8e8dd29c10706be45a4f5712b81be859d22c1f13))
- **repo:** Remove dyndns feature - ([6e33015](https://github.com/beyondessential/bestool/commit/6e33015d33b66bcfcdc5321209442bbd4da78797))
- **repo:** Fix release commit message format - ([87333b2](https://github.com/beyondessential/bestool/commit/87333b23c885f0a8b660a8e082962ead97e9121a))
- **repo:** Omit useless stuff from cliff - ([a52c24b](https://github.com/beyondessential/bestool/commit/a52c24b6b95bbd0ae995aa13fd2a348c6fb77ca8))
- **repo:** Completely remove dyndns - ([8bacd55](https://github.com/beyondessential/bestool/commit/8bacd55cacd14315599e69a3875543ce3262a7d9))
- **repo:** Remove useless file_chunker - ([16d55f5](https://github.com/beyondessential/bestool/commit/16d55f556b0a171b56f80e4bcfa8eccf07266955))
- **repo:** Add walg feature back so builds don’t break - ([f5908df](https://github.com/beyondessential/bestool/commit/f5908dfb863bbd7327b82dabc1e4e83ad3e934e6))
- **style:** Don't mix tokio and std io - ([bb6e07c](https://github.com/beyondessential/bestool/commit/bb6e07c9ef4a7b68b72592f9f04fad69135227b9))
- **style:** Remove a warning - ([03932be](https://github.com/beyondessential/bestool/commit/03932be80b2dfc80eb7fcdf84923a88a92c5ae23))
- **style:** Fix clippy - ([a5a571f](https://github.com/beyondessential/bestool/commit/a5a571f9d27177941978f934a8f508991918a344))
- **style:** Use proper types and traits - ([30ce373](https://github.com/beyondessential/bestool/commit/30ce3734bf8206a2723f4f1ec82c1004235b8a32))
- **test:** Add integration tests to bestool - ([3306f79](https://github.com/beyondessential/bestool/commit/3306f79f552ba7e598dbadc481a133b0169157fa))
- **test:** Fix - ([0d9b13b](https://github.com/beyondessential/bestool/commit/0d9b13b06791311a811bb8d50af2a076d70b809f))
- **tweak:** Use cache-busting URLs for downloads - ([25477c4](https://github.com/beyondessential/bestool/commit/25477c4eda21e73ab0a088bdf71a04d2de1d4895))
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

- **doc(algae):** Clarify naming - ([5a30692](https://github.com/beyondessential/bestool/commit/5a306923aeacd249ea3034209b591c657c81d963))
- **doc(algae):** Address google's "translation" - ([78ccc25](https://github.com/beyondessential/bestool/commit/78ccc25b67165fb87065889d5f2700ee94555a3f))
- **doc(algae):** Show example of encrypted identity - ([5501140](https://github.com/beyondessential/bestool/commit/5501140d336c01911c73bf5952ebda5dea311fcf))
- **feat(algae):** Extract crypto interface into its own tool/lib - ([3a6c194](https://github.com/beyondessential/bestool/commit/3a6c1942f7d9aace04274a7baeb2cec20a016603))
- **feat(algae):** Use pinentry when available - ([8edcb3c](https://github.com/beyondessential/bestool/commit/8edcb3cf267734093d4c19d2162a4f9b9fd204f0))
- **fix(algae):** Use algae as the name of the executable - ([2dea3bb](https://github.com/beyondessential/bestool/commit/2dea3bbcb8ce237bda10cc5265b54be9d11b9620))
- **fix(algae):** Actually print passphrase - ([ea7ca15](https://github.com/beyondessential/bestool/commit/ea7ca15e6fde130fa99890c9576d6b431ff6e124))
- **test(algae):** Fix doctests - ([b856439](https://github.com/beyondessential/bestool/commit/b85643984b0fc4bd19b7f78610d77cb50c12bda4))

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

- **feat(caddy):** Add configure-tamanu command - ([c3bbf89](https://github.com/beyondessential/bestool/commit/c3bbf89aa28e87dd013b79dc88ad6c0ed19bf0d1))
- **fix(caddy):** Create download path folder if non-extant - ([454c9fb](https://github.com/beyondessential/bestool/commit/454c9fb326320cdf64857ae5cda9402efdef74d7))
- **fix(caddy):** Make downloaded caddy executable on unix - ([4e89419](https://github.com/beyondessential/bestool/commit/4e89419d436516f6c932a5a0e37ca3620a0a94a5))

- **feat(cli):** Enable unambiguous shorthands - ([312cca9](https://github.com/beyondessential/bestool/commit/312cca9dada2c769235758c8efa6767b3fd2eca7))

- **feat(completions):** Make completions command optional - ([eee272d](https://github.com/beyondessential/bestool/commit/eee272d0fdab8721ca7c1496fbe9dc2bfda617e3))

- **doc(crypto):** Explain how to use the identity file in keygen - ([f7c77af](https://github.com/beyondessential/bestool/commit/f7c77afda4c586c9178d5d5c864582272238e65b))
- **doc(crypto):** Fix description of keygen - ([374d4ae](https://github.com/beyondessential/bestool/commit/374d4aeae0c6d4e7b7a600c9490399400c7c517e))
- **feat(crypto):** Add hash subcommand - ([ed71866](https://github.com/beyondessential/bestool/commit/ed7186679ff89dcdf3cba4158ef904680dbf7fa7))
- **feat(crypto):** KAM-297: add encrypt, decrypt, and keygen (#169) - ([a5367c3](https://github.com/beyondessential/bestool/commit/a5367c3c239045ea09a4336e3308fbd64d1bcddf))
- **feat(crypto):** Add protect/reveal commands for passphrase encryption - ([75f8e1d](https://github.com/beyondessential/bestool/commit/75f8e1d35aa01e27099822d0e77a96e75701317f))
- **feat(crypto):** Encrypt identity files by default - ([84061c3](https://github.com/beyondessential/bestool/commit/84061c307c028996fb5222d696cc7f569687363c))
- **feat(crypto):** Support encrypted identity files directly while en/decrypting - ([abc86a8](https://github.com/beyondessential/bestool/commit/abc86a8464820905a814215d9afa11b37b61eea6))
- **feat(crypto):** Add --rm to encrypt and protect - ([8828421](https://github.com/beyondessential/bestool/commit/88284210a5fa0ad2dce97f208248b9ab4adbcc70))
- **feat(crypto):** Write identity.pub by default - ([a39d39d](https://github.com/beyondessential/bestool/commit/a39d39d67f7e8ad7c7777cfc1732471c1ee249a9))
- **fix(crypto):** Zero the password after handling - ([dd6d3b3](https://github.com/beyondessential/bestool/commit/dd6d3b3d9c08d6322d07dbfdbbe90275bd894cda))
- **refactor(crypto):** Rename sign command to crypto - ([ab6b74e](https://github.com/beyondessential/bestool/commit/ab6b74ef752f0645ae48cda210fe86c58d8d398c))
- **refactor(crypto):** Rename check subcommand to verify - ([89b5684](https://github.com/beyondessential/bestool/commit/89b56845d0472006af12c1493a2b31e1d1a43ac6))
- **refactor(crypto):** Remove minisign subcommands - ([4cb17c6](https://github.com/beyondessential/bestool/commit/4cb17c608b00b420818228c0ac143293c1227ecb))
- **refactor(crypto):** Extract en/decryption and key handling routines - ([f7c77af](https://github.com/beyondessential/bestool/commit/f7c77afda4c586c9178d5d5c864582272238e65b))
- **refactor(crypto):** Use algae-cli in bestool - ([347af7a](https://github.com/beyondessential/bestool/commit/347af7ac6e05fdc50005abd5ca70eb5ae7a89a88))

- **fix(db-url):** Handle the case where a reporting username is empty in config - ([8af528a](https://github.com/beyondessential/bestool/commit/8af528a6a71e2630a10b154d77aad7c4e11f6fd5))

- **deps(deps):** Bump clap_complete_nushell from 4.4.2 to 4.5.0 (#10) - ([3036394](https://github.com/beyondessential/bestool/commit/3036394eb15e349580acd516646ea3983c3ae432))
- **deps(deps):** Bump aws-sdk-route53 from 1.13.0 to 1.13.1 (#11) - ([51314fc](https://github.com/beyondessential/bestool/commit/51314fc16b5b9cf29a48d2e0debd109087d2b65a))
- **deps(deps):** Bump serde_json from 1.0.111 to 1.0.113 (#12) - ([e7992c7](https://github.com/beyondessential/bestool/commit/e7992c78042103272a45652a47b2317693b05cc3))
- **deps(deps):** Bump clap_complete from 4.4.9 to 4.5.0 (#13) - ([edba885](https://github.com/beyondessential/bestool/commit/edba8852ce68012de405f90c79789c35cabb3dc5))

- **fix(downloads):** Use full tailscale name for alternative sources - ([ebb6522](https://github.com/beyondessential/bestool/commit/ebb6522d90c0af3837f2b166c0524a0434457f46))
- **fix(downloads):** Query tailscale dns directly to avoid buggy systems - ([63dccad](https://github.com/beyondessential/bestool/commit/63dccadcd8a2b2efe6ee14e57d19e356e0ef59eb))

- **feat(dyndns):** Add dyndns command for iti - ([492aa2d](https://github.com/beyondessential/bestool/commit/492aa2d449baab39b31c2d6a7eecf8e871211964))
- **fix(dyndns):** Type ambiguity on Windows - ([85dcb10](https://github.com/beyondessential/bestool/commit/85dcb1024a3a9d130c4712e7ef4ec281e678eb70))

- **feat(eink):** Add eink subcommand - ([474efd7](https://github.com/beyondessential/bestool/commit/474efd7adab0bf39e158e8c6c2fc2f01b7b932ef))
- **feat(eink):** Disable eink and dyndns by default - ([9a35e32](https://github.com/beyondessential/bestool/commit/9a35e32a9d52a9cc21348a6a4f017fad81661f41))

- **feat(greenmask):** Support multiple config directories - ([c02a898](https://github.com/beyondessential/bestool/commit/c02a8980b33159a04510e13f987598a5b9645e02))
- **feat(greenmask):** Look into release folder by default too - ([26ab927](https://github.com/beyondessential/bestool/commit/26ab9278786e562a404818137d79d6412a04a52d))
- **feat(greenmask):** Create storage dir if missing - ([1ddc568](https://github.com/beyondessential/bestool/commit/1ddc568f63a147a50f000dbb778b2755d954fc16))
- **fix(greenmask):** Default all paths - ([fc30309](https://github.com/beyondessential/bestool/commit/fc303094694e37b9d692a2cf4ab94bbe678ca60c))
- **fix(greenmask):** Correct storage stanza - ([604d184](https://github.com/beyondessential/bestool/commit/604d18416441c801bc6cd68013017d1646597451))
- **fix(greenmask):** Use dunce canonicalize instead of unc - ([476844f](https://github.com/beyondessential/bestool/commit/476844f835832ea1cdb789dd4392eae856854a02))

- **doc(iti):** Warn against using trace logging with lcd serve - ([56e3b0c](https://github.com/beyondessential/bestool/commit/56e3b0c636575b784cac3021e2cd455435d21b8f))
- **feat(iti):** Add commands for battery and lcd (#64) - ([56e3b0c](https://github.com/beyondessential/bestool/commit/56e3b0c636575b784cac3021e2cd455435d21b8f))
- **feat(iti):** Add systemd services for battery and lcd - ([56e3b0c](https://github.com/beyondessential/bestool/commit/56e3b0c636575b784cac3021e2cd455435d21b8f))
- **feat(iti):** Add systemd services for lcd display (#75) - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **feat(iti):** Add temperature to lcd - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **feat(iti):** Add local time to lcd - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **feat(iti):** Add network addresses to lcd - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **feat(iti):** Add wifi network to lcd - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **feat(iti):** Sparklines for cpu/ram usage - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **fix(iti):** Properly clear lcd on start and stop - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **fix(iti):** Omit podman network addresses - ([642a28a](https://github.com/beyondessential/bestool/commit/642a28aff69a1715581ef4beeed3fbfd4fe57f53))
- **refactor(iti):** Move eink and wifisetup into iti command - ([56e3b0c](https://github.com/beyondessential/bestool/commit/56e3b0c636575b784cac3021e2cd455435d21b8f))
- **refactor(iti):** Simplify bg/fg colour calculations - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **refactor(iti):** Remove wifisetup wip command - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **refactor(iti):** Pass upper args through - ([8e1837c](https://github.com/beyondessential/bestool/commit/8e1837cfb82c04de915ace644cf1e0399a56ae46))
- **tweak(iti):** Make time less precise for battery display - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **tweak(iti):** More responsive battery display - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))
- **tweak(iti):** Add fully charged message - ([ee0b7b7](https://github.com/beyondessential/bestool/commit/ee0b7b762a2d337dc7446dc6b417785103d57a8b))

- **feat(iti-addresses):** Include up to 3 ips - ([812909c](https://github.com/beyondessential/bestool/commit/812909c9773f6c27077ba7122dd740a6c3a8f0cf))

- **feat(psql):** Add read-only mode - ([fee043c](https://github.com/beyondessential/bestool/commit/fee043c29a3d85ed8acf76a59f9c37cd070da72a))
- **feat(psql):** Invert read/write default, require -W, --write to enable write mode - ([d947eb6](https://github.com/beyondessential/bestool/commit/d947eb65130e1b004507e33e877069fa48be487b))
- **feat(psql):** Arbitrary program and args - ([efed90a](https://github.com/beyondessential/bestool/commit/efed90a4431058e0efe230f43b88ee883a4f160e))
- **feat(psql):** Cull history database on open if it exceeds 100MB - ([ff41c81](https://github.com/beyondessential/bestool/commit/ff41c8108a804ec8a4fa8e786b5bcec155ae58fd))
- **feat(psql):** Compact history database on exit - ([369b66e](https://github.com/beyondessential/bestool/commit/369b66e9e5531813093318e8aaac95e313685607))
- **feat(psql):** Support concurrent writers to history database - ([dfdeeb1](https://github.com/beyondessential/bestool/commit/dfdeeb1c88967721633d2ea8324ac1ac5230fbd4))
- **feat(psql):** Read terminal size and initialize PTY with correct dimensions - ([c82f264](https://github.com/beyondessential/bestool/commit/c82f264319d8eafe3162eefefc6077067dd3e7a7))
- **feat(psql):** Respond to terminal resize events (SIGWINCH on Unix) - ([e086e6e](https://github.com/beyondessential/bestool/commit/e086e6e2a001e24f1fbde2067ec7fadfaa6b9b7b))
- **feat(psql):** Add Windows console resize event handling - ([f98a5e2](https://github.com/beyondessential/bestool/commit/f98a5e2506dd3bf587fd01d53811da6422db751c))
- **feat(psql):** Periodically reload history timestamps every 60 seconds - ([dea7344](https://github.com/beyondessential/bestool/commit/dea7344b5085b75d84788da23249e40d2bc3e8a0))
- **feat(psql):** Replace eprintln with tracing and add lloggs for logging - ([a87df2a](https://github.com/beyondessential/bestool/commit/a87df2a1ecdff1ca5948d32f08adf81184bb2e68))
- **feat(psql):** Implement editor - ([6c38f60](https://github.com/beyondessential/bestool/commit/6c38f60b59bf31ef306fac874e01e74ad45fd03d))
- **feat(psql):** Allow pager through pty - ([ffce074](https://github.com/beyondessential/bestool/commit/ffce074ea4eaeb964e984dde978d424c21fb8424))
- **feat(psql):** Syntax highlighting - ([67e93d4](https://github.com/beyondessential/bestool/commit/67e93d4c10f3aad2aeb034136ecea5e72da6d186))
- **fix(psql):** Disable pager in controlled mode - ([959b59f](https://github.com/beyondessential/bestool/commit/959b59f15df6327a1333e88cb5b5d895fa421a2c))
- **fix(psql):** Pager search cosmetics - ([d1ed9b4](https://github.com/beyondessential/bestool/commit/d1ed9b4474897cbf7a916be346d931be91cdd9f9))
- **tweak(psql):** Turn autocommit off when -W is given - ([b7636ce](https://github.com/beyondessential/bestool/commit/b7636ce3b197fb2301d329a9ce74db90ccfb3eb8))
- **tweak(psql):** Use UTF-8 codepage on Windows and force UTF8 encoding on PSQL - ([797fb83](https://github.com/beyondessential/bestool/commit/797fb835e66c5522f0c111aaa292474c4b253b44))
- **tweak(psql):** Default to \timing on - ([68843f2](https://github.com/beyondessential/bestool/commit/68843f2ab3c74acb7c1abce83e805ea8aad5df8b))
- **tweak(psql):** Use reporting users when present - ([0bcb91f](https://github.com/beyondessential/bestool/commit/0bcb91f10b457aa9bc5ae7d12ad66acb12511542))
- **tweak(psql):** Allow customising the codepage on windows - ([ffd3aff](https://github.com/beyondessential/bestool/commit/ffd3aff57c5b26450c68662b3a4ec47ceb15ab5c))

- **fix(s3):** Add to cleanup list - ([018d4d3](https://github.com/beyondessential/bestool/commit/018d4d3b523362d2c0ae24cf523c96dc7a4ecd7a))

- **feat(self-update):** Print version after upgrading - ([d3350b7](https://github.com/beyondessential/bestool/commit/d3350b739b0170bc68bb4ff85442910df0f385fd))
- **feat(self-update):** Add ourselves to PATH on windows with -P - ([c5e4651](https://github.com/beyondessential/bestool/commit/c5e4651628215960175a57ff7837810c0c18785e))
- **fix(self-update):** Try to self-update better - ([304cd24](https://github.com/beyondessential/bestool/commit/304cd2466dc8e8916e964da23c422cf49ab145f0))

- **feat(sign):** Add sign command - ([5917d85](https://github.com/beyondessential/bestool/commit/5917d85e2f7dff52b14dee735bdc75cf3ea932d4))
- **feat(sign):** Add check command - ([fdb12d1](https://github.com/beyondessential/bestool/commit/fdb12d1c7beaa6404a9f51ba659ffbe0b79ab50a))
- **feat(sign):** Add keygen command - ([ffbd9b5](https://github.com/beyondessential/bestool/commit/ffbd9b5a61d34c5b7ae7b83692a56f89e93ba208))
- **fix(sign):** Replace {n} placeholder with {num} - ([d993a9d](https://github.com/beyondessential/bestool/commit/d993a9db6f2667ca68234953576d6bfac89b548e))
- **fix(sign):** Signatures filenames would use foo..sig if the input didn't have an extension - ([648efd6](https://github.com/beyondessential/bestool/commit/648efd6a3e47d71e94405d6a4ddba739cd23eada))
- **refactor(sign):** Extract --output file resolving to reuse into check - ([b02a7b8](https://github.com/beyondessential/bestool/commit/b02a7b8aa2fe2144c84ce2aeb1df7d0c33d15b84))

- **feat(ssh):** Add add-key command (#39) - ([420bd46](https://github.com/beyondessential/bestool/commit/420bd4688952e34e906decbf1ce6cf3e96d79ee8))

- **doc(tamanu):** Fix docstring for tamanu download - ([4b36867](https://github.com/beyondessential/bestool/commit/4b368678c444d48aa671d1673e927aad881de603))
- **feat(tamanu):** Add download subcommand - ([b69e127](https://github.com/beyondessential/bestool/commit/b69e127c9b26e739413dc31dec92b32b1c23acc7))
- **feat(tamanu):** KAM-10: add psql command (#33) - ([b85a4c4](https://github.com/beyondessential/bestool/commit/b85a4c4ee2a02fd851808e7dee330357ff63d1d4))
- **feat(tamanu):** KAM-197: add prepare-upgrade (#38) - ([b292226](https://github.com/beyondessential/bestool/commit/b292226c33080bed5849db1e5f2dcc7e39084b34))
- **feat(tamanu):** KAM-198: add `upgrade` command (#40) - ([82a0545](https://github.com/beyondessential/bestool/commit/82a05457f34a053d40adc8c677bebf40c96ad476))
- **feat(tamanu):** Add greenmask-config command - ([4e874de](https://github.com/beyondessential/bestool/commit/4e874de4c1c61ae9c3c0ebd082d0a430bfc07eca))
- **feat(tamanu):** Add postgres backup tool (#137) - ([4f4c549](https://github.com/beyondessential/bestool/commit/4f4c549c3c996911d19a15b348e1562970fb07fd))
- **feat(tamanu):** Command to list artifacts from meta (#212) - ([d5ead2e](https://github.com/beyondessential/bestool/commit/d5ead2e87f7b0abbfac4fa4f1e55268d9cc6bcf1))
- **feat(tamanu):** Remove unused upgrade and pre-upgrade commands - ([d5ead2e](https://github.com/beyondessential/bestool/commit/d5ead2e87f7b0abbfac4fa4f1e55268d9cc6bcf1))
- **feat(tamanu):** Add dburl command - ([7eb2c90](https://github.com/beyondessential/bestool/commit/7eb2c9079d7393d6ee2b09f0d95b880e927e0ef6))
- **feat(tamanu):** Use new psql tool - ([fb5b6ae](https://github.com/beyondessential/bestool/commit/fb5b6ae49dbdb5b9093cb6b2810a19aab0bab777))
- **fix(tamanu):** Web package doesn't have a platform - ([eb830e4](https://github.com/beyondessential/bestool/commit/eb830e48ab6d271d0a0024c2c12ee22c3d105ade))
- **fix(tamanu):** Run pm2 directly - ([a84fe64](https://github.com/beyondessential/bestool/commit/a84fe643aaab374c140f387fd8ce0dee87bdddf2))
- **fix(tamanu):** Look into the right places for Linux installs' config - ([7b87111](https://github.com/beyondessential/bestool/commit/7b871115a518b12acc1a4ec2d64683ac6e3c00eb))
- **fix(tamanu):** Assume facility if we can't detect server type - ([78f605e](https://github.com/beyondessential/bestool/commit/78f605ef92b8edb8962f49ce6c850c591edd5e14))
- **fix(tamanu):** Windows compilation - ([12fdd8b](https://github.com/beyondessential/bestool/commit/12fdd8b3a651d4f42c06192ba399be05166c3631))
- **fix(tamanu):** Do not require mailgun until needed - ([99496d2](https://github.com/beyondessential/bestool/commit/99496d258b921892c1fcf439bd55275ecf01057d))
- **refactor(tamanu):** Move roots so tamanu deps can be optional - ([302b287](https://github.com/beyondessential/bestool/commit/302b287138e7ebc24a56cbe1218cb82a325d8cce))
- **tweak(tamanu):** Make download command able to download any artifact - ([d5ead2e](https://github.com/beyondessential/bestool/commit/d5ead2e87f7b0abbfac4fa4f1e55268d9cc6bcf1))
- **tweak(tamanu):** Support port field - ([2407714](https://github.com/beyondessential/bestool/commit/24077145d5ff64153fd7abc83a378efc5913b79d))
- **tweak(tamanu):** Move dburl command to url for mnemonics - ([e38cfcc](https://github.com/beyondessential/bestool/commit/e38cfccc0dae4b30f371c56a3306a6a1687e8b2e))

- **feat(upload):** Implement multipart uploads - ([2d99f9e](https://github.com/beyondessential/bestool/commit/2d99f9e9d66092ba23372f718278618eb45b28b1))
- **feat(upload):** Implement singlepart uploads - ([64ffe4c](https://github.com/beyondessential/bestool/commit/64ffe4c5cd11280c41b33d7f11fcde693676994d))
- **feat(upload):** Parse humantime durations - ([be6b117](https://github.com/beyondessential/bestool/commit/be6b117698d40d517434bbcf1bc1b72b2456baad))
- **feat(upload):** Encode tokens - ([ecac321](https://github.com/beyondessential/bestool/commit/ecac3215b9bacaf7e295202628bbe14069c15855))
- **feat(upload):** Cancel with upload-id - ([371f0f8](https://github.com/beyondessential/bestool/commit/371f0f880931eb8a13d696b56e95fe23697fba10))
- **feat(upload):** Implement preauth upload - ([318add7](https://github.com/beyondessential/bestool/commit/318add70cb7264e38b754f74134fb906daea9627))
- **feat(upload):** Prepare other upload commands - ([fe34663](https://github.com/beyondessential/bestool/commit/fe34663ffda86ea377291410412bfb5f11081d14))
- **feat(upload):** Attempt delegated tokens - ([f233dec](https://github.com/beyondessential/bestool/commit/f233dec3f796d365cf1375716bbb3d2457579cc3))
- **refactor(upload):** Split aws uploads - ([279e123](https://github.com/beyondessential/bestool/commit/279e123e7c21bd547a06b2bbc3a117c99c245e03))
- **refactor(upload):** Common aws args - ([5cd77a2](https://github.com/beyondessential/bestool/commit/5cd77a244d57a4d71561e6c6fecd47f185829b77))
- **test(upload):** Add tests and fix handling of bucket/key arguments - ([5f8a5ea](https://github.com/beyondessential/bestool/commit/5f8a5ea05fcbdad439879cb5d4a6a5204af37bc9))

- **tweak(url):** Don't include empty password if no password is provided - ([9117dd5](https://github.com/beyondessential/bestool/commit/9117dd5a32231002f00c577b367ad1deaa3185ac))

- **feat(wal-g):** Add wal-g download command - ([bc84c68](https://github.com/beyondessential/bestool/commit/bc84c68c47e29f80f7cedfa604c54aa1db2f402a))

- **feat(wifisetup):** Implement scan - ([f83e7eb](https://github.com/beyondessential/bestool/commit/f83e7eba0433e2e1af2179a181d76ea8320b7718))
- **refactor(wifisetup):** Revise interface for nm - ([440c377](https://github.com/beyondessential/bestool/commit/440c37740dd0710d8cbc0e8ed62b2e3c5b169fd7))
<!-- generated by git-cliff -->
