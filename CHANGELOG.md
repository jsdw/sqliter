# 0.8.0

- **Breaking:** Opening a connection is now split into two steps, so that you can be explicit about whether to perform migrations.
  - `ConnectionBuilder::open` and `ConnectionBuilder::open_in_memory` are renamed to `ConnectionBuilder::database` and `ConnectionBuilder::in_memory`, and now return a `ConnectionPendingMigrations`.
  - `ConnectionPendingMigrations::open()` performs any necessary migrations (and sets the app ID on a new database) before returning the `Connection`. Dropping the `ConnectionPendingMigrations` instead leaves the database unmodified.
  - `ConnectionPendingMigrations::needs_migrations()` and `ConnectionPendingMigrations::is_out_of_date()` can be used to inspect the database before deciding what to do.
- Fix migrations sometimes being applied out of order when they were not added in ascending version order.
- **Breaking:** Adding a migration with the same version as an existing one now panics.
- **Breaking:** `ConnectionBuilderError` is replaced by `ConnectionError` (opening the database) and `MigrationsError` (performing migrations). Both convert into the new overarching `Error` type.

# 0.7.0

- Update to `async-rusqlite` 0.6.0 (and rusqlite 0.41.0 as a result).
- Update to Rust 2024 edition; MSRV is now 1.89.

# 0.6.0

- Update to `async-rusqlite` 0.5.0 (and rusqlite 0.37.0 as a result).

# 0.5.1

- Move tempfile dependency to dev dependencies (not sure when/how it ended up as an actual one!)

# 0.5.0

- Allow non-transactional migrations to be performed via `Migrations::add_non_transactionally()`.

# 0.4.0

- Allow `on_close` handler to be attached when constructing our database connection.

# 0.3.0

- Panic if migration with version 0 or less given (it wouldn't work as hoped).
- Add a bunch of tests.

# 0.2.0

- `Migrations` also now has a default error parameter.
- `Migrations::add` consumes/returns `self` now for slightly nicer usage.

# 0.1.1

- The `Error` parameter on `ConnectionBuilder` now defaults to `rusqlite::Error`

# 0.1.0

Initial release.