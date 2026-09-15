# L2: dispatch each check with a context for its own subject

Build a check's context for the one subject it reports for, split the check
signature into a machine arm and an application arm, and retire the registry's
category axis.

## Shapes

```rust
pub struct MachineCx {
    http: reqwest::Client,
    canopy: Option<Arc<CanopyClient>>,
    tamanu: Option<MachineTamanu>,
}

pub struct MachineTamanu { version: Version, root: Option<PathBuf> }

pub struct AppCx {
    app: ApplicationRef,
    version: Version,
    kind: ApiServerKind,
    config: Arc<TamanuConfig>,
    install_root: Option<PathBuf>,
    database_url: String,
    pool: Option<PgPool>,
    http: reqwest::Client,
}

pub struct Runner<Cx> {
    run: fn(Cx) -> BoxFuture<'static, Check>,
    heal: Option<HealAction<Cx>>,
}

pub enum Run {
    Machine(Runner<MachineCx>),
    Application(AppScope, Runner<AppCx>),
}

pub struct CheckEntry { name, on_wire, run: Run }
```

`AppCx` carries no canopy client: the only heal that reaches canopy is
`canopy_registration`, which is a machine check. That is a real tightening, not
an omission.

## Why `MachineCx` carries a Tamanu

Two machine checks grade machine artefacts that Tamanu's installer put there,
and need to know which Tamanu is on the box to grade them:

- `disk_free` considers the mount holding the install root, alongside `/`.
- `caddyfile_version` grades the machine's Caddyfile marker against the
  deployment's version.

Both are machine checks under `SUBJ` — the front-end configuration file and the
machine's filesystems are the machine's. "What is installed on this machine" is
a machine fact, not a reading taken from the application, so `MachineCx` carries
it rather than either check changing subject. `MachineTamanu` is deliberately
thin: a version and an install root, no config, no database, no application key.

This is a deviation from the card's illustrative `MachineCx { store, http,
canopy }`, which does not account for those two checks. `store` is K1's.

## Retiring the second axis

`is_tamanu` goes entirely. Its information becomes the presence or absence of a
Tamanu application: a host with only a generic `DATABASE_URL` resolves a
Postgres application and no Tamanu one, so every Tamanu-scoped check is already
absent by scope. The only place the old category arm's `is_tamanu` skip was
still reachable was `caddyfile_version` (scope machine, category tamanu), whose
skip moves into the check itself alongside its existing "not Windows" and "caddy
not present" skips — which `CHK-CFV` requires it to keep.

`has_install` becomes `install_root: Option<PathBuf>` on `AppCx` and `root:
Option<PathBuf>` on `MachineTamanu`: a property of the subject rather than a
gate. `AppCx::installed_config()` returns the config only where there are
install files to have read it from.

`SweepTamanu` splits into `SweepTargets { database_url, tamanu: Option<SweepTamanu> }`.

## Heal keying

`heal::spawn_if_due` keys its rate limit and its one-attempt-in-flight guard on
the qualified name (`tamanu-central:fhir_jobs`), not the bare check name, so two
applications' heals for one check do not share a limit. The registry key becomes
`String`.

## Build steps

- [x] `AppScope` replaces `CheckScope`; `Machine` variant removed, `admits` takes an `ApplicationRef`
- [x] `MachineCx`, `MachineTamanu`, `AppCx` defined; `SweepContext` and `CheckContext` removed
- [x] `Runner<Cx>`, `Run`, `HealAction<Cx>`; `CheckEntry` carries `run: Run`
- [x] `entry!` macro loses its category argument
- [x] Machine checks take `MachineCx` (16 modules)
- [x] Application checks take `AppCx` (30 modules)
- [x] `caddyfile_version` moves to `MachineCx` and skips on its own when no Tamanu
- [x] `disk_free` reads the install root from `MachineCx`
- [x] `caddy_certs`, `http_errors` move from `SweepContext` to `AppCx`
- [x] `fhir_jobs::heal` takes `AppCx`; `canopy_registration::heal` takes `MachineCx`
- [x] `heal` keyed on the qualified name; `HealAction` generic over its context
- [x] `SweepTargets` replaces `SweepTamanu`'s `is_tamanu`/`has_install`
- [x] Sweep builds one `AppCx` per application and dispatches per subject
- [x] `bestool` call sites updated
- [x] Tests updated; `test_support` yields `AppCx`
- [x] `cargo clippy`, `cargo fmt`, tests on Linux and a Windows target

## Settled in review

- **The heal key is the instance form** (`postgres-5432:connect`), not
  `Subject::qualify`'s type-level selection name. Keying on the selection name
  collapsed two clusters onto one rate limit and one in-flight slot, which is
  the opposite of what `CHK` requires. Derived through a named `heal_key` so a
  regression is testable.
- **A Postgres cluster's context carries none of the Tamanu's parameters.**
  `install_root` is what `installed_config` keys on, so inheriting the Tamanu's
  root made a cluster answer with another application's configuration.
- **The heal expands inside the registry arm**, so a cross-arm heal is a type
  error where it is written rather than a startup panic.
- `kind` is the one field with no neutral value in a shared `AppCx`. Every
  check that reads it is Tamanu-scoped, so none reaches what a cluster carries
  there. Splitting `AppCx` per application kind would remove the last of this,
  but that changes the card's `Run` shape and is not in scope here.

## Notes

- The database connection is carried through as today: one shared pool, cloned
  into each application's `AppCx`. Whether each application gets its own
  connection is `M1`'s.
- `enable_heal` leaves the context entirely. It was never a check's input — it
  gates dispatch, and `perform_sweep` already takes it as an argument.
