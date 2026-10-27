use async_rusqlite::Connection;
use std::path::Path;

use crate::MigrationsError;
use crate::error::ConnectionError;
use crate::migrations::Migrations;

/// An opinionated connection builder which ultimately hands back
/// an [`async_rusqlite::Connection`] after checking the app ID and
/// performing any necessary migrations.
pub struct ConnectionBuilder<E = rusqlite::Error> {
    // PRAGMA application_id = INTEGER; default 0
    app_id: i32,
    // Migrations to apply
    migrations: Migrations<E>,
    // Function to call when the db thread shuts down
    on_close: Option<Box<dyn FnOnce(Option<rusqlite::Connection>) + Send + 'static>>,
}

impl<E: Send + 'static> Default for ConnectionBuilder<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: Send + 'static> ConnectionBuilder<E> {
    /// Construct a new connection builder.
    pub fn new() -> Self {
        Self {
            app_id: 0,
            migrations: Default::default(),
            on_close: None,
        }
    }

    /// Configure a function to be called exactly once when the connection is closed.
    /// If the database has already been closed then it will be given `None`, else it
    /// will be handed the database connection.
    pub fn on_close<F: FnOnce(Option<rusqlite::Connection>) + Send + 'static>(
        mut self,
        f: F,
    ) -> Self {
        self.on_close = Some(Box::new(f));
        self
    }

    /// Set the "app ID" for this database. If opening an existing file,
    /// this Id must match else an error will be generated. This helps to
    /// ensure that the database we're trying to open is meant for the app
    /// we're running. Default to 0 if not set ("SQLite Database").
    pub fn app_id(mut self, app_id: i32) -> Self {
        self.app_id = app_id;
        self
    }

    /// Add a single migration to the list, which will be responsible for
    /// upgrading the database to the version given.
    ///
    /// # Panics
    ///
    /// Panics if the migration version given is not greater than 0.
    pub fn add_migration<F>(mut self, version: i32, migration: F) -> Self
    where
        F: Send + 'static + Fn(&rusqlite::Connection) -> Result<(), E>,
    {
        self.migrations = self.migrations.add(version, migration);
        self
    }

    /// **Warning: using this could lead to database state being invalid.**
    ///
    /// Add a single migration to the list. The migration will not be performed
    /// inside a transaction. Use [`Self::add_migration`] unless you know what
    /// you are doing.
    pub fn add_migration_non_transactionally<F>(mut self, version: i32, migration: F) -> Self
    where
        F: Send + 'static + Fn(&rusqlite::Connection) -> Result<(), E>,
    {
        self.migrations = self.migrations.add_non_transactionally(version, migration);
        self
    }

    /// Use the provided set of migrations to ensure that the database we connect
    /// to is uptodate. This uses the `user_version` PRAGMA to know which migrations
    /// to apply.
    pub fn set_migrations(mut self, migrations: Migrations<E>) -> Self {
        self.migrations = migrations;
        self
    }

    /// Prepare to open a connection to an in-memory database.
    pub async fn in_memory(mut self) -> Result<ConnectionPendingMigrations<E>, ConnectionError> {
        let conn = self
            .connection_builder()
            .open_in_memory()
            .await
            .map_err(ConnectionError::Db)?;

        let current_db_version = self.setup(&conn, true).await?;
        Ok(ConnectionPendingMigrations::new(
            conn,
            self.migrations,
            current_db_version,
        ))
    }

    /// Prepare to open a connection to a database file at some path.
    pub async fn database<P: AsRef<Path>>(
        mut self,
        path: P,
    ) -> Result<ConnectionPendingMigrations<E>, ConnectionError> {
        use async_rusqlite::rusqlite::{
            Error::SqliteFailure, OpenFlags, ffi, ffi::ErrorCode::CannotOpen,
        };

        // The default flags rusqlite's open fn uses. First we try opening
        // and disallow creating a new DB. Then we allow creating a new DB.
        // This allows us to know when a new DB was created and act accordingly.
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;

        let (conn, is_new) = match self
            .connection_builder()
            .open_with_flags(path.as_ref(), flags)
            .await
        {
            // All good:
            Ok(conn) => (conn, false),
            // Can't open the file; try again but allow creating it:
            Err(SqliteFailure(
                ffi::Error {
                    code: CannotOpen, ..
                },
                _,
            )) => {
                let flags = flags | OpenFlags::SQLITE_OPEN_CREATE;
                let conn = self
                    .connection_builder()
                    .open_with_flags(path, flags)
                    .await
                    .map_err(ConnectionError::Db)?;
                (conn, true)
            }
            // Something else went wrong; just return the error.
            Err(e) => return Err(ConnectionError::Db(e)),
        };

        let current_db_version = self.setup(&conn, is_new).await?;
        Ok(ConnectionPendingMigrations::new(
            conn,
            self.migrations,
            current_db_version,
        ))
    }

    // A connection builder.
    fn connection_builder(&mut self) -> async_rusqlite::ConnectionBuilder {
        let mut builder = Connection::builder();

        if let Some(on_close) = self.on_close.take() {
            builder = builder.on_close(on_close);
        }

        builder
    }

    // Perform any setup on the opened connection.
    async fn setup(&self, conn: &Connection, is_new: bool) -> Result<i32, ConnectionError> {
        let app_id = self.app_id;
        conn.call(move |conn| {
            if is_new {
                // Set up the app ID if this is a new DB.
                conn.pragma_update(None, "application_id", app_id)
                    .map_err(ConnectionError::Db)?;
            } else {
                // Check the app ID if this is not a new DB.
                let val: i32 = conn
                    .query_row("SELECT * from pragma_application_id", [], |row| row.get(0))
                    .map_err(ConnectionError::Db)?;
                if val != app_id {
                    return Err(ConnectionError::WrongApplicationId(val));
                }
            }

            // Set foreign key constraint checking.
            conn.pragma_update(None, "foreign_keys", true)
                .map_err(ConnectionError::Db)?;

            // Which version is the DB at (ie do we need to run any migrations)
            let user_version: i32 = conn
                .query_row("SELECT * FROM pragma_user_version", [], |row| row.get(0))
                .map_err(ConnectionError::Db)?;

            Ok(user_version)
        })
        .await
    }
}

/// A connection that has been opened but may still require migrations
/// to be completed in order to be usable.
pub struct ConnectionPendingMigrations<E = rusqlite::Error> {
    // Migrations to apply
    migrations: Migrations<E>,
    // The current migration version we're up to in the DB (also 'user_version')
    current_db_version: i32,
    // The latest migration version our migrations take us up to
    latest_migration_version: i32,
    // The underlying DB connection
    conn: Connection,
}

impl<E: Send + 'static> ConnectionPendingMigrations<E> {
    // Crate-only function to init.
    fn new(conn: Connection, migrations: Migrations<E>, current_db_version: i32) -> Self {
        // Which version are the given migrations at?
        let latest_migration_version = migrations
            .iter()
            .map(|(version, _, _)| version)
            .max()
            .unwrap_or(0);

        ConnectionPendingMigrations {
            migrations,
            current_db_version,
            latest_migration_version,
            conn,
        }
    }

    /// Are the migrations we know about behind the current database?
    pub fn is_out_of_date(&self) -> bool {
        self.latest_migration_version < self.current_db_version
    }

    /// Does this connection need migrations in order to open?
    pub fn needs_migrations(&self) -> bool {
        self.current_db_version < self.latest_migration_version
    }

    /// Hand back a [`Connection`] without performing
    /// migrations, returning `None` if there are migrations that
    /// must be performed before the connection is ready, or if the
    /// migrations we know about are not enough to align to the current
    /// database version.
    pub fn open_without_migrations(self) -> Option<Connection> {
        if self.current_db_version == self.latest_migration_version {
            Some(self.conn)
        } else {
            None
        }
    }

    /// Hand back a [`Connection`] after having performed
    /// the necessary migrations, returning an error if something
    /// related to migrating goes wrong.
    pub async fn open(self) -> Result<Connection, MigrationsError<E>> {
        let current_db_version = self.current_db_version;

        // If our migrations are out of date, bail without doing anything.
        if self.is_out_of_date() {
            return Err(MigrationsError::OutOfDate {
                db_version: current_db_version,
                latest_migration: self.latest_migration_version,
            });
        }

        // If we need migrations, run them.
        if self.needs_migrations() {
            let migrations = self.migrations;
            self.conn
                .call(move |conn| {
                    // Attempt each migration atomically. If a migration fails, we don't
                    // want the DB to have been altered.
                    for (version, perform_in_transaction, migration) in migrations.iter() {
                        if version > current_db_version {
                            if perform_in_transaction {
                                // in one transaction, apply a migration and update the db version
                                // to reflect this. nothing happens on failure; transaction rolled back.
                                let transaction = conn.transaction()?;
                                migration(&transaction).map_err(|e| {
                                    MigrationsError::MigrationFailed { version, reason: e }
                                })?;
                                transaction.pragma_update(None, "user_version", version)?;
                                transaction.commit()?;
                            } else {
                                // This is less safe, since any failure inside the migration can lead to
                                // the database being in an invalid state. Sometimes though, we need to
                                // control the transaction behaviour inside the migration, so this is
                                // the best we can do.
                                migration(conn).map_err(|e| MigrationsError::MigrationFailed {
                                    version,
                                    reason: e,
                                })?;
                                conn.pragma_update(None, "user_version", version)?;
                            }
                        }
                    }

                    Ok::<_, MigrationsError<E>>(())
                })
                .await?;
        }

        Ok(self.conn)
    }
}
