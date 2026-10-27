/// Any error in `sqliter` can convert into this overarching type.
#[derive(Debug, thiserror::Error)]
pub enum Error<E = rusqlite::Error> {
    #[error("Cannot build the connection: {0}")]
    Connection(#[from] ConnectionError),
    #[error("Cannot migrate the database: {0}")]
    Migrations(#[from] MigrationsError<E>),
}

/// An error relating to the migrations.
#[derive(Debug, thiserror::Error)]
pub enum MigrationsError<E = rusqlite::Error> {
    #[error("Connection unexpectedly closed during migrations")]
    UnexpectedlyClosed,
    #[error("App out of date; database at version {db_version} but app works with version {latest_migration}")]
    OutOfDate {
        db_version: i32,
        latest_migration: i32,
    },
    #[error("Migration to version {version} failed: {reason}")]
    MigrationFailed {
        version: i32,
        reason: E
    },
    #[error("Database error: {0}")]
    Db(rusqlite::Error),
}

impl <E> From<async_rusqlite::AlreadyClosed> for MigrationsError<E> {
    fn from(_value: async_rusqlite::AlreadyClosed) -> Self {
        MigrationsError::UnexpectedlyClosed
    }
}

impl <E> From<rusqlite::Error> for MigrationsError<E> {
    fn from(value: rusqlite::Error) -> Self {
        MigrationsError::Db(value)
    }
}

/// An error relating to the database or connection to it.
#[derive(Debug, thiserror::Error)]
pub enum ConnectionError {
    #[error("Connection unexpectedly closed")]
    UnexpectedlyClosed,
    #[error("Wrong application ID: got {0}")]
    WrongApplicationId(i32),
    #[error("Database error: {0}")]
    Db(rusqlite::Error),
}

impl From<async_rusqlite::AlreadyClosed> for ConnectionError {
    fn from(_value: async_rusqlite::AlreadyClosed) -> Self {
        ConnectionError::UnexpectedlyClosed
    }
}

impl From<rusqlite::Error> for ConnectionError {
    fn from(value: rusqlite::Error) -> Self {
        ConnectionError::Db(value)
    }
}