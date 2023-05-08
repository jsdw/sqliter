
#[derive(Debug)]
#[non_exhaustive]
pub enum ConnectionBuilderError<E = rusqlite::Error> {
    UnexpectedlyClosed,
    WrongApplicationId(i32),
    OutOfDate { db_version: i32, latest_migration: i32 },
    Db(rusqlite::Error),
    Migration(E)
}

impl <E: std::fmt::Display> std::fmt::Display for ConnectionBuilderError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionBuilderError::UnexpectedlyClosed =>
                write!(f, "Connection unexpectedly closed"),
            ConnectionBuilderError::WrongApplicationId(n) =>
                write!(f, "Wrong application ID; got {n}"),
            ConnectionBuilderError::OutOfDate { db_version, latest_migration } =>
                write!(f, "App out of date; database at version {db_version} but app works with version {latest_migration}"),
            ConnectionBuilderError::Db(err) =>
                write!(f, "Database error: {err}"),
            ConnectionBuilderError::Migration(err) =>
                write!(f, "Migration error: {err}")
        }
    }
}

impl <E: std::error::Error + 'static> std::error::Error for ConnectionBuilderError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ConnectionBuilderError::UnexpectedlyClosed |
            ConnectionBuilderError::WrongApplicationId(_) |
            ConnectionBuilderError::OutOfDate { .. } => None,
            ConnectionBuilderError::Db(err) => Some(err),
            ConnectionBuilderError::Migration(err) => Some(err),
        }
    }
}

impl <E> From<rusqlite::Error> for ConnectionBuilderError<E> {
    fn from(value: rusqlite::Error) -> Self {
        ConnectionBuilderError::Db(value)
    }
}

impl <E> From<async_rusqlite::AlreadyClosed> for ConnectionBuilderError<E> {
    fn from(_value: async_rusqlite::AlreadyClosed) -> Self {
        ConnectionBuilderError::UnexpectedlyClosed
    }
}