use sqlx::migrate::Migrator;
use sqlx::sqlite::SqlitePoolOptions;
use std::env::var;
use std::path::Path;

#[tokio::main]
async fn main() {
    // TODO: maybe a nicer error lol
    let sqlite_db_path = dotenv::var("SQLITE_DB_PATH").unwrap();
    // use a SqliteConnectOptions instead of a hardcoded queryparam?
    let path_with_params = format!("{}?mode=rwc", sqlite_db_path);
    let pool = SqlitePoolOptions::new()
        .max_connections(12)
        .connect(&path_with_params)
        .await
        .unwrap();
    let migrator = Migrator::new(Path::new("./migrations")).await.unwrap();

    let migrated = migrator.run(&pool).await;
    match migrated {
        Ok(()) => {}
        Err(e) => {
            println!("cargo:warning=Migration error: {:?}", e);
            panic!("Failed to run migrations");
        }
    }
}
