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