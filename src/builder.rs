use std::path::Path;
use async_rusqlite::{Connection};

use crate::migrations::Migrations;
use crate::error::ConnectionBuilderError;

/// An opinionated connection builder which ultimately hands back
/// an [`async_rusqlite::Connection`] after checking the app ID and
/// performing any necessary migrations.
pub struct ConnectionBuilder<E = rusqlite::Error> {
    // PRAGMA application_id = INTEGER; default 0
    app_id: i32,
    // Migrations to apply
    migrations: Migrations<E>,
    // Function to call when the db thread shuts down
    on_close: Option<Box<dyn FnOnce(Option<rusqlite::Connection>) + Send + 'static>>
}

impl <E: Send + 'static> ConnectionBuilder<E> {
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
    pub fn on_close<F: FnOnce(Option<rusqlite::Connection>) + Send + 'static>(mut self, f: F) -> Self {
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
        F: Send + 'static + Fn(&rusqlite::Connection) -> Result<(), E>
    {
        self.migrations = self.migrations.add(version, migration);
        self
    }

    /// Use the provided set of migrations to ensure that the database we connect
    /// to is uptodate. This uses the `user_version` PRAGMA to know which migrations
    /// to apply.
    pub fn set_migrations(mut self, migrations: Migrations<E>) -> Self {
        self.migrations = migrations;
        self
    }

    /// Open a connection to an in-memory database.
    pub async fn open_in_memory(mut self) -> Result<Connection, ConnectionBuilderError<E>> {
        let mut conn = self.connection_builder().open_in_memory().await?;
        self.setup(&mut conn, true).await?;
        Ok(conn)
    }

    /// Open a connection to a database at some file.
    pub async fn open<P: AsRef<Path>>(mut self, path: P) -> Result<Connection, ConnectionBuilderError<E>> {
        use async_rusqlite::rusqlite::{
            OpenFlags, Error::SqliteFailure, ffi::ErrorCode::CannotOpen, ffi
        };

        // The default flags rusqlite's open fn uses. First we try opening
        // and disallow creating a new DB. Then we allow creating a new DB.
        // This allows us to know when a new DB was created and act accordingly.
        let flags
            = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;

        let (conn, is_new) = match self.connection_builder().open_with_flags(path.as_ref(), flags).await {
            // All good:
            Ok(conn) => (conn, false),
            // Can't open the file; try again but allow creating it:
            Err(SqliteFailure(ffi::Error { code, .. }, _)) if code == CannotOpen => {
                let flags = flags | OpenFlags::SQLITE_OPEN_CREATE;
                let conn = self.connection_builder().open_with_flags(path, flags).await?;
                (conn, true)
            },
            // Something else went wrong; just return the error.
            Err(e) => return Err(e.into()),
        };

        self.setup(&conn, is_new).await?;
        Ok(conn)
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
    async fn setup(self, conn: &Connection, is_new: bool) -> Result<(), ConnectionBuilderError<E>> {
        conn.call(move |conn| {
            if is_new {
                // Set up the app ID if this is a new DB.
                conn.pragma_update(None, "application_id", self.app_id)?;
            } else {
                // Check the app ID if this is not a new DB.
                let val: i32 = conn.query_row(
                    "SELECT * from pragma_application_id",
                    [],
                    |row| row.get(0)
                )?;
                if val != self.app_id {
                    return Err(ConnectionBuilderError::WrongApplicationId(val))
                }
            }

            // Set foreign key constraint checking.
            conn.pragma_update(None, "foreign_keys", true)?;

            // Which version is the DB at (ie do we need to run any migrations)
            let user_version: i32 = conn.query_row(
                "SELECT * FROM pragma_user_version",
                [],
                |row| row.get(0)
            )?;

            // Attempt all migrations, doing none on failure. Main reason for this is
            // to update the user_version PRAGMA on success and ensure that either everything
            // inc that version is in sync always.
            let transaction = conn.transaction()?;

            let mut latest_migration_version = 0;
            for (version, migration) in self.migrations.iter() {
                latest_migration_version = version;
                if version > user_version {
                    migration(&*transaction).map_err(ConnectionBuilderError::Migration)?;
                }
            }

            if latest_migration_version > user_version {
                // Some migrations happened; update user version and commit transaction.
                transaction.pragma_update(None, "user_version", latest_migration_version)?;
                transaction.commit()?;
            } else if latest_migration_version < user_version {
                // We don't have migrations up to the version that the db is at already.
                // This probably means that this app is out of date. Complain, to prevent
                // an out of date app from trying to use the newer database.
                return Err(ConnectionBuilderError::OutOfDate {
                    db_version: user_version,
                    latest_migration: latest_migration_version
                }.into())
            }

            Ok(())
        }).await
    }
}
