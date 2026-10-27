# Sqliter

[![Crates.io](https://img.shields.io/crates/v/sqliter.svg)](https://crates.io/crates/sqliter)
[![Docs.rs](https://docs.rs/sqliter/badge.svg)](https://docs.rs/sqliter)
[![CI](https://github.com/jsdw/sqliter/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/jsdw/sqliter/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/crates/l/sqliter.svg)](https://github.com/jsdw/sqliter/blob/main/Cargo.toml)

A small, opinionated wrapper around Rusqlite which handles migrations for you, and exposes a small amount of configuration around instantiating Sqlite connections.

Built on [`async-rusqlite`](https://crates.io/crates/async-rusqlite); a thin async wrapper around [`rusqlite`](https://crates.io/crates/rusqlite) that is runtime agnostic.

## Example

```rust
use sqliter::{Connection, ConnectionBuilder, Error};

async fn open_connection() -> Result<Connection, Error> {
    // This must never change for a given app; the database won't open if
    // the app_id does not equal the one we have set here.
    const APP_ID: i32 = 1337;

    // Use `.database(path)` instead of `.in_memory()` to open a database file.
    let pending = ConnectionBuilder::new()
        .app_id(APP_ID)
        // Migrations should never change; this list will simply grow over time
        // to accommodate the updates needed as the app matures.
        .add_migration(1, |conn| {
            conn.execute("CREATE TABLE user ( id INTEGER PRIMARY KEY )", ())
                .map(|_| ())
        })
        .add_migration(2, |conn| {
            conn.execute(
                "ALTER TABLE user ADD COLUMN name TEXT NOT NULL DEFAULT 'Unknown'",
                (),
            )
            .map(|_| ())
        })
        .in_memory()
        .await?;

    // Migrations are not run until we ask for them. Here we could check
    // `pending.needs_migrations()` and, for instance, back up the database
    // first, or drop `pending` to leave the database unmodified.
    let conn = pending.open().await?;
    Ok(conn)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let conn = open_connection().await?;

    conn.call(|conn| conn.execute("INSERT INTO user (name) VALUES ('James')", ()))
        .await?;

    Ok(())
}
```
