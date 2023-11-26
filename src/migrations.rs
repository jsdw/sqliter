use std::collections::BinaryHeap;
use std::cmp::Reverse;

type MigrationFn<E> = dyn Send + 'static + Fn(&rusqlite::Connection) -> Result<(), E>;

/// Define a set of migrations to apply to an SQLite connection.
pub struct Migrations<E = rusqlite::Error> {
    migrations: BinaryHeap<Reverse<Migration<E>>>
}

impl <E> Default for Migrations<E> {
    fn default() -> Self {
        Migrations::new()
    }
}

impl <E> Migrations<E> {
    /// Construct a new set of migrations,
    pub fn new() -> Self {
        Migrations {
            migrations: Default::default()
        }
    }

    /// Add a migration to the list with an associated version number.
    /// Migrations are run in order from lowest to highest number, and the
    /// user_version is set in the DB to record the version we're up to.
    /// Migrations should never be removed or changed once they have been
    /// applied to a DB somewhere.
    ///
    /// # Panics
    ///
    /// Panics if the migration version given is not greater than 0.
    pub fn add<F>(self, version: i32, migration: F) -> Self
    where F: Fn(&rusqlite::Connection) -> Result<(), E> + Send + 'static
    {
        self.do_add_migration(version, true, migration)
    }

    /// Like [`Migrations::add()`], except the migration will _not_ be performed
    /// inside a transaction. **This can lead to an invalid database state; use
    /// with extreme caution**.
    ///
    /// The expected use case for this is that it will internally begin a transaction,
    /// but may choose to do things like disabling foreign keys first, which cannot
    /// be done inside a transaction.
    pub fn add_non_transactionally<F>(self, version: i32, migration: F) -> Self
    where F: Fn(&rusqlite::Connection) -> Result<(), E> + Send + 'static
    {
        self.do_add_migration(version, false, migration)
    }

    fn do_add_migration<F>(mut self, version: i32, perform_in_transaction: bool, migration: F) -> Self
    where F: Fn(&rusqlite::Connection) -> Result<(), E> + Send + 'static
    {
        assert!(version > 0, "migration version must be greater than 0");
        let migration = Box::new(migration);
        self.migrations.push(Reverse(Migration {
            version,
            perform_in_transaction,
            migration
        }));
        self
    }

    /// Iterate over the migrations, lowest to highest version.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (i32, bool, &MigrationFn<E>)> {
        self.migrations.iter().map(|Reverse(m)| (m.version, m.perform_in_transaction, &*m.migration))
    }
}

/// Migrations are ordered by their version.
struct Migration<E> {
    version: i32,
    perform_in_transaction: bool,
    migration: Box<MigrationFn<E>>
}

impl <E> PartialEq for Migration<E> {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
    }
}

impl <E> Eq for Migration<E> {}

impl <E> Ord for Migration<E> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.version.cmp(&other.version)
    }
}

impl <E> PartialOrd for Migration<E> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.version.partial_cmp(&other.version)
    }
}