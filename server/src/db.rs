use anyhow::Result;
use include_dir::{include_dir, Dir};
use lazy_static::lazy_static;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::Connection;
use rusqlite_migration::Migrations;
use std::path::PathBuf;

static MIGRATIONS_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/migrations");

lazy_static! {
    static ref MIGRATIONS: Migrations<'static> =
        Migrations::from_directory(&MIGRATIONS_DIR).unwrap();
}

pub fn init_db(path: &PathBuf) -> Result<Pool<SqliteConnectionManager>> {
    let manager = SqliteConnectionManager::file(path);
    let pool = r2d2::Pool::builder().max_size(64).build(manager)?;

    let mut conn = pool.get()?;

    set_pragma(&mut conn)?;
    migrate(&mut conn)?;

    Ok(pool)
}

pub fn set_pragma(conn: &mut Connection) -> Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    
    let mmap_size = std::env::var("LRCLIB_MMAP_SIZE")
        .ok()
        .and_then(|val| val.parse::<i64>().ok())
        .unwrap_or(30000000000i64)
        .to_string();
    conn.pragma_update(None, "mmap_size", &mmap_size)?;
    
    let cache_size = std::env::var("LRCLIB_CACHE_SIZE")
        .ok()
        .and_then(|val| val.parse::<i64>().ok())
        .unwrap_or(-1000000i64)
        .to_string();
    conn.pragma_update(None, "cache_size", &cache_size)?;

    Ok(())
}

pub fn migrate(conn: &mut Connection) -> Result<()> {
    MIGRATIONS.to_latest(conn)?;
    Ok(())
}
