
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Wrong application ID: got {0}")]
    WrongApplicationId(i32),
    #[error("Database error: {0}")]
    Db(#[from] rusqlite::Error)
}