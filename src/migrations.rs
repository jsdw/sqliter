use std::collections::BinaryHeap;
use std::cmp::Reverse;
use std::sync::Arc;

type MigrationFn<Ctx> = dyn Fn(&rusqlite::Connection, &Ctx) -> rusqlite::Result<()>;

/// Define a set of migrations to apply to an SQLite connection.
#[derive(Clone)]
pub struct Migrations<Ctx> {
    migrations: BinaryHeap<Reverse<Migration<Ctx>>>
}

impl <Ctx> Default for Migrations<Ctx> {
    fn default() -> Self {
        Migrations::new()
    }
}

impl <Ctx> Migrations<Ctx> {
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
    pub fn add<F>(&mut self, version: i32, migration: F) -> &mut Self
    where F: Fn(&rusqlite::Connection, &Ctx) -> rusqlite::Result<()> + 'static
    {
        let migration = Arc::new(migration);
        self.migrations.push(Reverse(Migration { version, migration }));
        self
    }

    /// Iterate over the migrations, lowest to highest version.
    pub fn iter(&self) -> impl Iterator<Item = (i32, &MigrationFn<Ctx>)> {
        self.migrations.iter().map(|Reverse(m)| (m.version, &*m.migration))
    }
}

/// Migrations are ordered by their version.
#[derive(Clone)]
struct Migration<Ctx> {
    version: i32,
    migration: Arc<MigrationFn<Ctx>>
}

impl <Ctx> PartialEq for Migration<Ctx> {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
    }
}

impl <Ctx> Eq for Migration<Ctx> {}

impl <Ctx> Ord for Migration<Ctx> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.version.cmp(&other.version)
    }
}

impl <Ctx> PartialOrd for Migration<Ctx> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        self.version.partial_cmp(&other.version)
    }
}