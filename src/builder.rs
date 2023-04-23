use std::path::Path;
use rusqlite::{Connection};
use std::marker::PhantomData;

use crate::migrations::Migrations;
use crate::error::Error;

/// A locked [`ConnectionBuilder`] contains this type.
pub enum Locked {}
/// An unlocked [`ConnectionBuilder`] contains this type.
pub enum Unlocked {}

/// An opinionated connection builder which ultimately hands back
/// a [`rusqlite::Connection`] after checking the app ID and performing
/// any necessary migrations.
pub struct ConnectionBuilder<Ctx, State = Unlocked> {
    /// Some arbitrary context to pass to migration functions.
    context: Ctx,
    // PRAGMA application_id = INTEGER; default 0
    app_id: i32,
    // PRAGMA foreign_keys = bool; default true
    enforce_foreign_keys: bool,
    // Migrations to apply
    migrations: Migrations<Ctx>,
    // Hold unused generics
    marker: PhantomData<State>
}

impl ConnectionBuilder<(), Unlocked> {
    /// Construct a new connection builder.
    pub fn new() -> Self {
        Self::new_with_context(())
    }
}

impl <Ctx> ConnectionBuilder<Ctx, Unlocked> {
    /// Construct a new connection builder, providing some context
    /// that will be shared with any migrations that need running.
    pub fn new_with_context(ctx: Ctx) -> Self {
        Self {
            app_id: 0,
            context: ctx,
            enforce_foreign_keys: true,
            migrations: Default::default(),
            marker: PhantomData
        }
    }

    /// Should foreign key constraints be enforced? Defaults to true.
    ///
    /// SQLite has had foreign keys constrinats for a long time, but did not
    /// enforce them until more recently. We enable this by default, but
    /// give the option to disable it here.
    pub fn enforce_foreign_keys(&mut self, b: bool) -> &mut Self {
        self.enforce_foreign_keys = b;
        self
    }

    /// Use the provided set of migrations to ensure that the database we connect
    /// to is uptodate. This uses the `user_version` PRAGMA to know which migrations
    /// to apply.
    pub fn set_migrations(&mut self, migrations: Migrations<Ctx>) -> &mut Self {
        self.migrations = migrations;
        self
    }

    /// Use this method to prevent subsequent updates to the database configuration.
    /// After using this, you can only instantiate new connections.
    pub fn lock(self) -> ConnectionBuilder<Ctx, Locked> {
        ConnectionBuilder {
            app_id: self.app_id,
            context: self.context,
            enforce_foreign_keys: self.enforce_foreign_keys,
            migrations: self.migrations,
            marker: PhantomData,
        }
    }
}

impl <Ctx, State> ConnectionBuilder<Ctx, State> {
    /// Open a connection to an in-memory database.
    pub fn open_in_memory(&self) -> Result<Connection, Error> {
        let mut conn = Connection::open_in_memory()?;
        self.setup(&mut conn, true)?;
        Ok(conn)
    }

    /// Open a connection to a database at some file.
    pub fn open<P: AsRef<Path>>(&self, path: P) -> Result<Connection, Error> {
        use rusqlite::{ OpenFlags, Error::SqliteFailure, ffi::ErrorCode::CannotOpen };

        // The default flags rusqlite's open fn uses. First we try opening
        // and disallow creating a new DB. Then we allow creating a new DB.
        // This allows us to know when a new DB was created and act accordingly.
        let flags
            = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;

        let (mut conn, is_new) = match Connection::open_with_flags(path.as_ref(), flags) {
            // All good:
            Ok(conn) => (conn, false),
            // Can't open the file; try again but allow creating it:
            Err(SqliteFailure(rusqlite::ffi::Error { code, .. }, _)) if code == CannotOpen => {
                let flags = flags | OpenFlags::SQLITE_OPEN_CREATE;
                let conn = Connection::open_with_flags(path, flags)?;
                (conn, true)
            },
            // Something else went wrong; just return the error.
            Err(e) => return Err(e.into()),
        };

        self.setup(&mut conn, is_new)?;
        Ok(conn)
    }

    // Perform any setup on the opened connection.
    fn setup(&self, conn: &mut Connection, is_new: bool) -> Result<(), Error> {
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
                return Err(Error::WrongApplicationId(val))
            }
        }

        // Set foreign key constraint checking.
        conn.pragma_update(None, "foreign_keys", self.enforce_foreign_keys)?;

        // Which version is the DB at (ie do we need to run any migrations)
        let user_version: i32 = conn.query_row(
            "SELECT * FROM pragma_user_version",
            [],
            |row| row.get(0)
        )?;

        // Attempt all migrations, do nothing on failure. Main reason for this is
        // to update the user_version PRAGMA on success and ensure that either everything
        // inc that version is in sync always.
        let transaction = conn.transaction()?;

        let mut latest_migration_version = user_version;
        for (version, migration) in self.migrations.iter() {
            if version > user_version {
                migration(&*transaction, &self.context)?;
                latest_migration_version = version;
            }
        }

        // Some migrations happened; update user version and commit transaction.
        if latest_migration_version > user_version {
            transaction.pragma_update(None, "user_version", latest_migration_version)?;
            transaction.commit()?;
        }

        Ok(())
    }
}