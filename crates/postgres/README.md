# bestool-postgres

PostgreSQL connection-pool utilities shared by the [BES tooling][repo]. Wraps
[`tokio-postgres`] with [`mobc`] pooling and [`rustls`] TLS, and adds a few
quality-of-life helpers we want everywhere.

[repo]: https://github.com/beyondessential/bestool
[`tokio-postgres`]: https://docs.rs/tokio-postgres
[`mobc`]: https://docs.rs/mobc
[`rustls`]: https://docs.rs/rustls

## What's in it

- `PgPool::create_pool(url, application_name)` — build a pool from a libpq
  connection URL. Handles a few things that bare `tokio-postgres` does not:
  - Unix-socket connection strings (`postgresql:///db?host=/var/run/postgresql`,
    percent-encoded host, or empty host with auto-detect).
  - `sslmode=prefer` fallback to disabled TLS when the server refuses.
  - Interactive password prompt (via `rpassword`) if the URL has no password
    and the server returns an auth error.
- `pg_interval::Interval` — a `Duration` newtype that implements `ToSql` for
  PostgreSQL's `INTERVAL` type.
- `stringify::postgres_to_json_value` — best-effort row-cell → `serde_json::Value`
  conversion across the common PG types.
- `text_cast` — extract any column as its text representation regardless of
  declared type, useful for generic dump/inspect tools.
- `error` — shared error types and a `miette`-friendly diagnostic style.

## Use

```toml
[dependencies]
bestool-postgres = "1"
```

```rust,no_run
use bestool_postgres::pool::create_pool;

# async fn run() -> miette::Result<()> {
let pool = create_pool("postgresql://localhost/mydb", "my-app").await?;
let conn = pool.get().await.unwrap();
conn.simple_query("SELECT 1").await.unwrap();
# Ok(()) }
```

## Scope

This crate exists to support BES tools (alertd, psql, bestool itself); it is
deliberately opinionated rather than a general-purpose pool. That said, the
helpers are reusable, and patches that broaden them sensibly are welcome.

## License

GPL-3.0-or-later.
