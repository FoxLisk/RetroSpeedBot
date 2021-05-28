use sqlx::migrate::Migrator;
use sqlx::sqlite::SqlitePoolOptions;
use std::path::Path;

#[tokio::main]
async fn main() {
    println!("cargo:rerun-if-env-changed=DATABASE_URL");
    println!("cargo:rerun-if-changed=migrations/");
    println!("cargo:rerun-if-changed=test_db.db3");
    // TODO: maybe a nicer error lol
    // TODO: this kind of sucks for portability. making some way to run migrations without running
    //       the build seems like a good value add.
    let sqlite_db_path = dotenv::var("DATABASE_URL").unwrap();
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
