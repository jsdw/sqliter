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
    /// Panics if the migration version given is not greater than 0, or if
    /// a migration with the same version has already been added.
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
    ///
    /// # Panics
    ///
    /// Panics if the migration version given is not greater than 0, or if
    /// a migration with the same version has already been added.
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
        let conn = self.connection_builder().open_in_memory().await?;
        self.setup(conn).await
    }

    /// Prepare to open a connection to a database file at some path.
    pub async fn database<P: AsRef<Path>>(
        mut self,
        path: P,
    ) -> Result<ConnectionPendingMigrations<E>, ConnectionError> {
        let conn = match self.connection_builder().open(path.as_ref()).await {
            // All good:
            Ok(conn) => conn,
            // Something went wrong; just return the error.
            Err(e) => return Err(ConnectionError::Db(e)),
        };
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
                    conn.query_row("SELECT * FROM pragma_user_version", [], |row| row.get(0))?;

                // A database is considered new if it has no schema, and is version 0.
                // if either of these are not true, we do not treat it as new.
                let is_new = schema_count == 0 && user_version == 0;

                // If the database is _not_ new, we check the app ID and complain.
                // If it _is_ new, then we'll set this at the start of performing
                // migrations.
                if !is_new {
                    // Check the app ID if this is not a new DB.
                    let val: i32 =
                        conn.query_row("SELECT * from pragma_application_id", [], |row| {
                            row.get(0)
                        })?;
                    if val != app_id {
                        return Err(ConnectionError::WrongApplicationId(val));
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
    ) -> Self {
        // Which version are the given migrations at?
        let latest_migration_version = migrations
            .iter()
            .map(|(version, _, _)| version)
            .max()
            .unwrap_or(0);

        ConnectionPendingMigrations {
            migrations,
            app_id_to_set,
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
