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

#[cfg(test)]
mod test {
    use super::*;

    fn users_table(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch("
            CREATE TABLE users (
                id INTEGER PRIMARY KEY NOT NULL,
                name TEXT NOT NULL
            ) STRICT;

            INSERT INTO users VALUES (1, 'James');
            INSERT INTO users VALUES (2, 'Bob');
            ",
        )?;
        Ok(())
    }

    fn data_table(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
        conn.execute_batch("
            CREATE TABLE data (
                owner INTEGER NOT NULL,
                text TEXT NOT NULL,

                FOREIGN KEY(owner) REFERENCES users(id)
            ) STRICT;

            INSERT INTO data VALUES (1, 'James data');
        ")
    }

    async fn get_app_id(conn: &Connection) -> i32 {
        conn.call(|conn| {
            conn.query_row(
                "SELECT * from pragma_application_id",
                [],
                |row| row.get(0)
            )
        }).await.unwrap()
    }

    fn get_user_version_rusqlite(conn: &rusqlite::Connection) -> i32 {
        conn.pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("user_version expected")
    }

    async fn get_user_version(conn: &Connection) -> i32 {
        conn.call(|conn| {
            Ok::<_, rusqlite::Error>(get_user_version_rusqlite(conn))
        }).await.unwrap()
    }

    #[tokio::test]
    #[should_panic]
    async fn invalid_migration_version() {
        ConnectionBuilder::new()
            .add_migration(0, users_table)
            .open_in_memory()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn migration_is_applied() {
        let conn = ConnectionBuilder::new()
            .add_migration(7, users_table)
            .open_in_memory()
            .await
            .unwrap();

        // version should align:
        assert_eq!(get_user_version(&conn).await, 7);

        // table should be accessible and contain what the migration added:
        let name: String = conn.call(|conn| {
            conn.query_row("SELECT name FROM users WHERE id = 1", [], |row| row.get(0))
        }).await.unwrap();
        assert_eq!(name, "James");
    }

    #[tokio::test]
    async fn new_migrations_applied_as_needed() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("test-db1.app");

        // open first time with 1 migration:
        let conn = ConnectionBuilder::new()
            .add_migration(1, users_table)
            .open(&path)
            .await
            .unwrap();

        assert_eq!(get_user_version(&conn).await, 1);
        drop(conn);

        // open again with 2 migrations:
        let conn = ConnectionBuilder::new()
            .add_migration(1, users_table)
            .add_migration(2, data_table)
            .open(&path)
            .await
            .unwrap();

        assert_eq!(get_user_version(&conn).await, 2);

        // Ensure both migrations applied:
        let id: i32 = conn.call(|conn| {
            conn.query_row("
                SELECT users.id FROM data JOIN users ON data.owner = users.id
                WHERE data.text = 'James data';
            ", [], |row| row.get(0))
        }).await.unwrap();
        assert_eq!(id, 1);
    }

    #[tokio::test]
    async fn app_id_is_set_and_used() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("test-db2.app");

        const APP_ID: i32 = 12345;
        const DIFFERENT_APP_ID: i32 = 76543;

        // Should be no issues on initial opening:
        let conn = ConnectionBuilder::new()
            .app_id(APP_ID)
            .add_migration(1, users_table)
            .open(&path)
            .await
            .expect("can open db");

        assert_eq!(get_app_id(&conn).await, APP_ID);
        drop(conn);

        // Open again with same app ID should work:
        let conn = ConnectionBuilder::new()
            .app_id(APP_ID)
            .add_migration(1, users_table)
            .open(&path)
            .await
            .expect("can open db");

        assert_eq!(get_app_id(&conn).await, APP_ID);
        drop(conn);

        // Open again with different app ID should fail:
        let conn = ConnectionBuilder::new()
            .app_id(DIFFERENT_APP_ID)
            .add_migration(1, users_table)
            .open(&path)
            .await;

        assert!(
            matches!(conn, Err(ConnectionBuilderError::WrongApplicationId(APP_ID)))
        );
    }

    #[tokio::test]
    async fn older_migrations_wont_work() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("test-db1.app");

        // open with 2 migrations:
        let conn = ConnectionBuilder::new()
            .add_migration(1, users_table)
            .add_migration(2, data_table)
            .open(&path)
            .await
            .unwrap();

        drop(conn);

        // open again with just one migration. This could happen
        // if eg older software tries opening newer db.
        let conn = ConnectionBuilder::new()
            .add_migration(1, users_table)
            .open(&path)
            .await;

        assert!(
            matches!(conn, Err(ConnectionBuilderError::OutOfDate { db_version: 2, latest_migration: 1 }))
        );
    }

    #[tokio::test]
    async fn migrations_applied_up_to_broken_one() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("test-db1.app");

        // open with 1 valid migrations:
        let conn = ConnectionBuilder::new()
            .add_migration(1, users_table)
            .open(&path)
            .await
            .unwrap();

        drop(conn);

        // open again with another 2 migrations, 1 valid
        // and 1 broken.
        let conn = ConnectionBuilder::new()
            .add_migration(1, users_table)
            .add_migration(2, data_table)
            .add_migration(3, |conn| {
                conn.execute("SOME GARBAGE", [])?;
                Ok(())
            })
            .open(&path)
            .await;

        assert!(
            matches!(conn, Err(ConnectionBuilderError::Migration(_)))
        );

        // Check that user version was set to 2:
        let conn = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(get_user_version_rusqlite(&conn), 2);

        // Opening db with valid migrations set should be fine:
        let conn = ConnectionBuilder::new()
            .add_migration(1, users_table)
            .add_migration(2, data_table)
            .open(&path)
            .await
            .unwrap();

        let data_call = conn.call(|conn| {
            conn.query_row("SELECT count(*) FROM data", [], |_| Ok(()))
        }).await;
        assert!(data_call.is_ok());
    }

    #[tokio::test]
    async fn non_transactional_migration_can_be_applied() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("test-db1.app");

        let conn = ConnectionBuilder::new()
            .add_migration_non_transactionally(1, |conn| {
                // Deliberately error after adding some bits to the db:
                users_table(conn).expect("should work");
                Err(rusqlite::Error::InvalidQuery)
            })
            .open(&path)
            .await;

        assert!(
            matches!(conn, Err(ConnectionBuilderError::Migration(_)))
        );

        // Check that user version was not updated:
        let conn = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(get_user_version_rusqlite(&conn), 0);

        // But, users table should be accessible and contain what the failed migration
        // added, since it wasn't in a transaction and thus wasnt rolled back.
        let name: String = conn
            .query_row("SELECT name FROM users WHERE id = 1", [], |row| row.get(0))
            .unwrap();
        assert_eq!(name, "James");
    }
}
