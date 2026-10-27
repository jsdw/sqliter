use async_rusqlite::Connection;
use std::path::Path;

use crate::MigrationsError;
use crate::error::ConnectionError;
use crate::migrations::Migrations;

/// An opinionated connection builder. Open an application database
/// [from a file][ConnectionBuilder::database] or
/// [in memory][ConnectionBuilder::in_memory] and run or check the migration
/// status of it.
pub struct ConnectionBuilder<E = rusqlite::Error> {
    // PRAGMA application_id = INTEGER; default 0
    app_id: i32,
    // Migrations to apply
    migrations: Migrations<E>,
    // Function to call when the db thread shuts down
    on_close: Option<Box<dyn FnOnce(Option<rusqlite::Connection>) + Send + 'static>>,
    // Should we set the journal mode to WAL?
    wal: bool,
}

impl<E> Default for ConnectionBuilder<E> {
    fn default() -> Self {
        Self {
            app_id: 0,
            migrations: Migrations::new(),
            on_close: None,
            wal: false,
        }
    }
}

impl ConnectionBuilder {
    /// Construct a new connection builder whose migrations return
    /// [`rusqlite::Error`]s.
    ///
    /// To use some other error type in migrations, construct the builder
    /// using [`ConnectionBuilder::default()`] instead.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl<E: Send + 'static> ConnectionBuilder<E> {
    /// Configure a function to be called exactly once when the connection is closed.
    /// If the database has already been closed then it will be given `None`, else it
    /// will be handed the database connection.
    #[must_use]
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
    #[must_use]
    pub fn app_id(mut self, app_id: i32) -> Self {
        self.app_id = app_id;
        self
    }

    /// Set the journal mode of the database to WAL and, if this succeeds, the
    /// `synchronous` PRAGMA to `NORMAL`. This is done when the connection is opened via
    /// [`ConnectionPendingMigrations::open`] or
    /// [`ConnectionPendingMigrations::open_without_migrating`], before any
    /// migrations are performed.
    ///
    /// The WAL journal mode is persisted in the database file, so once a
    /// database has been opened with this, it will remain in WAL mode even if
    /// it's opened without this in the future. The `synchronous` PRAGMA is not
    /// persisted, and applies only to this connection.
    ///
    /// `synchronous = NORMAL` is the recommended setting in WAL mode: the database
    /// cannot be corrupted, but the most recent transactions may be lost on power
    /// failure or OS crash (though not if the application crashes). If you'd prefer
    /// otherwise, set `synchronous` yourself once the connection is opened.
    ///
    /// SQLite does not complain if WAL cannot be enabled, and so neither do we.
    /// For example, in-memory databases cannot use WAL and will be left in the
    /// `memory` journal mode, and WAL may not be available on some network
    /// filesystems or for read-only databases. In these cases, `synchronous` is
    /// left unchanged, since `NORMAL` carries a small risk of database corruption
    /// on power failure in non-WAL modes.
    #[must_use]
    pub fn with_wal(mut self) -> Self {
        self.wal = true;
        self
    }

    /// Add a single migration to the list, which will be responsible for
    /// upgrading the database to the version given.
    ///
    /// # Panics
    ///
    /// Panics if the migration version given is not greater than 0, or if
    /// a migration with the same version has already been added.
    #[must_use]
    pub fn add_migration<F>(mut self, version: i32, migration: F) -> Self
    where
        F: Send + 'static + FnOnce(&rusqlite::Connection) -> Result<(), E>,
    {
        self.migrations = self.migrations.add(version, migration);
        self
    }

    /// **Warning: using this could lead to database state being invalid.**
    ///
    /// Add a single migration to the list. The migration will not be performed
    /// inside a transaction. Use [`Self::add_migration`] unless you know what
    /// you are doing. See [`Migrations::add_non_transactionally`] for more.
    ///
    /// # Panics
    ///
    /// Panics if the migration version given is not greater than 0, or if
    /// a migration with the same version has already been added.
    #[must_use]
    pub fn add_migration_non_transactionally<F>(mut self, version: i32, migration: F) -> Self
    where
        F: Send + 'static + FnOnce(&rusqlite::Connection) -> Result<(), E>,
    {
        self.migrations = self.migrations.add_non_transactionally(version, migration);
        self
    }

    /// Use the provided set of migrations to ensure that the database we connect
    /// to is up to date. This uses the `user_version` PRAGMA to know which migrations
    /// to apply.
    ///
    /// This replaces any migrations previously given to this builder, including any
    /// added via [`Self::add_migration`] or [`Self::add_migration_non_transactionally`].
    #[must_use]
    pub fn set_migrations(mut self, migrations: Migrations<E>) -> Self {
        self.migrations = migrations;
        self
    }

    /// Prepare to open a connection to an in-memory database.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be opened or inspected.
    pub async fn in_memory(mut self) -> Result<ConnectionPendingMigrations<E>, ConnectionError> {
        let conn = self.connection_builder().open_in_memory().await?;
        self.setup(conn).await
    }

    /// Prepare to open a connection to a database file at some path.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be opened or inspected, or if
    /// it is not new and its app ID does not match [`Self::app_id`].
    pub async fn database<P: AsRef<Path>>(
        mut self,
        path: P,
    ) -> Result<ConnectionPendingMigrations<E>, ConnectionError> {
        let conn = self.connection_builder().open(path.as_ref()).await?;
        self.setup(conn).await
    }

    // A connection builder.
    fn connection_builder(&mut self) -> async_rusqlite::ConnectionBuilder {
        let mut builder = Connection::builder();

        if let Some(on_close) = self.on_close.take() {
            builder = builder.on_close(on_close);
        }

        builder
    }

    // Perform any setup on the opened connection and return
    // a connection pending migrations being applied.
    async fn setup(
        self,
        conn: Connection,
    ) -> Result<ConnectionPendingMigrations<E>, ConnectionError> {
        let app_id = self.app_id;
        let (current_db_version, is_new) = conn
            .call(move |conn| {
                // How much schema has been defined so far?
                let schema_count: i32 =
                    conn.query_row("SELECT count(*) FROM sqlite_schema", [], |row| row.get(0))?;

                // Which version is the DB at?
                let user_version: i32 =
                    conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

                // A database is considered new if it has no schema, and is version 0.
                // if either of these are not true, we do not treat it as new.
                let is_new = schema_count == 0 && user_version == 0;

                // If the database is _not_ new, we check the app ID and complain.
                // If it _is_ new, then we'll set this at the start of performing
                // migrations.
                if !is_new {
                    // Check the app ID if this is not a new DB.
                    let found: i32 =
                        conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
                    if found != app_id {
                        return Err(ConnectionError::WrongApplicationId {
                            expected: app_id,
                            found,
                        });
                    }
                }

                // Set foreign key constraint checking.
                conn.pragma_update(None, "foreign_keys", true)?;

                Ok((user_version, is_new))
            })
            .await?;

        Ok(ConnectionPendingMigrations::new(
            conn,
            self.migrations,
            current_db_version,
            is_new.then_some(self.app_id),
            self.wal,
        ))
    }
}

/// A connection that has been opened but may still require migrations
/// to be completed in order to be usable.
pub struct ConnectionPendingMigrations<E = rusqlite::Error> {
    // Migrations to apply
    migrations: Migrations<E>,
    // If this is provided, then we need to set the app ID. This
    // means that the database is new and does not have it configured yet.
    app_id_to_set: Option<i32>,
    // The current migration version we're up to in the DB (also 'user_version')
    current_db_version: i32,
    // The latest migration version our migrations take us up to
    latest_migration_version: i32,
    // Should we set the journal mode to WAL on opening?
    wal: bool,
    // The underlying DB connection
    conn: Connection,
}

impl<E: Send + 'static> ConnectionPendingMigrations<E> {
    // Crate-only function to init.
    fn new(
        conn: Connection,
        migrations: Migrations<E>,
        current_db_version: i32,
        app_id_to_set: Option<i32>,
        wal: bool,
    ) -> Self {
        let latest_migration_version = migrations.latest_version();

        ConnectionPendingMigrations {
            migrations,
            app_id_to_set,
            current_db_version,
            latest_migration_version,
            wal,
            conn,
        }
    }

    /// Are the migrations we know about behind the current database?
    #[must_use]
    pub fn is_out_of_date(&self) -> bool {
        self.latest_migration_version < self.current_db_version
    }

    /// Does this connection need migrations in order to open?
    #[must_use]
    pub fn needs_migrations(&self) -> bool {
        self.current_db_version < self.latest_migration_version
    }

    /// Hand back a [`Connection`] without performing any migrations.
    /// This will return [`None`] if migrations are needed. Aside from enabling
    /// WAL if [`ConnectionBuilder::with_wal`] was used (which does nothing if
    /// WAL is already enabled), this does not write to the database, so it is
    /// safe to call this multiple times in parallel against the same database file.
    ///
    /// # Errors
    ///
    /// Returns an error if WAL was asked for and setting it fails.
    pub async fn open_without_migrating(self) -> Result<Option<Connection>, ConnectionError> {
        // If our migrations are out of date, bail without doing anything.
        if self.current_db_version != self.latest_migration_version {
            return Ok(None);
        }

        self.enable_wal_if_needed().await?;
        Ok(Some(self.conn))
    }

    /// Hand back a [`Connection`] after having performed
    /// the necessary migrations, returning an error if something
    /// related to migrating goes wrong.
    ///
    /// # Warning
    ///
    /// Do not run this against the same database more than once at a
    /// time; doing so risks migrations being applied multiple times or
    /// out of order. If you need to open multiple instances of the
    /// same database, use [`Self::open_without_migrating`].
    pub async fn open(self) -> Result<Connection, MigrationsError<E>> {
        let current_db_version = self.current_db_version;

        // If our migrations are out of date, bail without doing anything.
        if self.is_out_of_date() {
            return Err(MigrationsError::OutOfDate {
                db_version: current_db_version,
                latest_migration: self.latest_migration_version,
            });
        }

        // If the database version isn't one we know about, then our migrations
        // probably don't belong to this database, so bail without doing anything.
        if current_db_version != 0 && !self.migrations.contains_version(current_db_version) {
            return Err(MigrationsError::UnknownVersion {
                db_version: current_db_version,
            });
        }

        // Enable WAL before anything else is written, so that migrations run in WAL mode.
        self.enable_wal_if_needed().await?;

        // If the database is new, we set up the App ID for it. We do this
        // regardless of whether any migrations will happen.
        if let Some(app_id) = self.app_id_to_set {
            self.conn
                .call(move |conn| conn.pragma_update(None, "application_id", app_id))
                .await?;
        }

        // If we need migrations, run them.
        if self.needs_migrations() {
            let migrations = self.migrations;
            self.conn
                .call(move |conn| {
                    // Attempt each migration atomically. If a migration fails, we don't
                    // want the DB to have been altered.
                    for (version, perform_in_transaction, migration) in migrations.into_ordered() {
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

                                // If the migration left a transaction open, then it hasn't really
                                // completed, so roll back whatever it left uncommitted and complain.
                                // We're already returning an error, so ignore any rollback error.
                                if !conn.is_autocommit() {
                                    let _ = conn.execute_batch("ROLLBACK");
                                    return Err(MigrationsError::TransactionLeftOpen { version });
                                }

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

    // Set the journal mode to WAL if this was asked for, and synchronous to NORMAL
    // if WAL was successfully enabled. Setting WAL does nothing if it is already
    // enabled. We don't complain if WAL could not be enabled.
    async fn enable_wal_if_needed(&self) -> Result<(), rusqlite::Error> {
        if self.wal {
            self.conn
                .call(|conn| {
                    // This returns the journal mode now in effect, which may not be WAL.
                    let mode: String =
                        conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| {
                            row.get(0)
                        })?;
                    if mode.eq_ignore_ascii_case("wal") {
                        conn.pragma_update(None, "synchronous", "NORMAL")?;
                    }
                    Ok::<_, rusqlite::Error>(())
                })
                .await?;
        }
        Ok(())
    }
}
