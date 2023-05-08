//! # Sqliter
//!
//! Make light work of connecting and migrating an SQLite database.
//!
//! Built on [`async_rusqlite`]; a thin async wrapper around [`rusqlite`]
//! that is runtime agnostic.
//!
//! ```rust
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use sqliter::{ Connection, ConnectionBuilder, ConnectionBuilderError };
//!
//! async fn open_connection() -> Result<Connection, ConnectionBuilderError> {
//!     // This must never change for a given app; the database won't open if
//!     // the app_id does not equal the one we have set here.
//!     const APP_ID: i32 = 1337;
//!
//!     ConnectionBuilder::new()
//!         .app_id(APP_ID)
//!         // Migrations should never change; this list will simply grow over time
//!         // To accomodate the updates needed as the app matures.
//!         .add_migration(1, |conn| {
//!             conn.execute("CREATE TABLE user ( id INTEGER PRIMARY KEY )", ())
//!                 .map(|_| ())
//!         })
//!         .add_migration(2, |conn| {
//!             conn.execute("ALTER TABLE user ADD COLUMN name TEXT NOT NULL DEFAULT 'Unknown'", ())
//!                 .map(|_| ())
//!         })
//!         .open_in_memory()
//!         .await
//! }
//!
//! let conn = open_connection().await?;
//!
//! conn.call(|conn| {
//!     conn.execute("INSERT INTO user (name) VALUES ('James')", ())
//! }).await?;
//!
//! # Ok(())
//! # }
//! ```

mod builder;
mod error;
mod migrations;

pub use builder::ConnectionBuilder;
pub use error::ConnectionBuilderError;
pub use migrations::Migrations;

// Export these since we are just a thin wrapper around them.
pub use async_rusqlite::{ self, rusqlite, Connection };
