mod builder;
mod error;
mod migrations;

pub use builder::{
    ConnectionBuilder,
    Locked,
    Unlocked
};
pub use error::Error;
pub use migrations::Migrations;

// Export this since we are just a thin wrapper around it.
pub use rusqlite;