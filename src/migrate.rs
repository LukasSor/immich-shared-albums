/** migrate.rs — moving the state between SQLite and Postgres, and refusing to start on a SQLite file that has gone stale. See ARCHITECTURE.md. */
use crate::db::{import, postgres::Postgres, Db};
use crate::store::{Store, StoreError};
use std::path::{Path, PathBuf};

const MARKER: &str = ".moved-to-postgres";

#[derive(Debug, PartialEq)]
pub enum Direction {
    ToPostgres,
    ToSqlite,
}

fn marker_path(data_dir: &str) -> PathBuf {
    Path::new(data_dir).join(MARKER)
}

fn state_db_path(data_dir: &str) -> PathBuf {
    Path::new(data_dir).join("state.db")
}

pub fn has_moved(data_dir: &str) -> bool {
    marker_path(data_dir).exists()
}

pub fn mark_moved(data_dir: &str) -> Result<(), StoreError> {
    std::fs::write(
        marker_path(data_dir),
        format!(
            "state.db was copied into Postgres at {}; it is out of date from then on\n",
            crate::config::iso_now()
        ),
    )
    .map_err(|e| StoreError::Io(e.to_string()))
}

pub fn clear_marker(data_dir: &str) {
    let _ = std::fs::remove_file(marker_path(data_dir));
}

pub fn refuse_stale_sqlite(data_dir: &str, allow: bool) -> Result<(), StoreError> {
    if !has_moved(data_dir) {
        return Ok(());
    }
    if !allow {
        return Err(StoreError::MovedToPostgres);
    }
    crate::log!(
        "starting on state.db although this install moved to Postgres (ISA_ALLOW_STALE_STATE_DB): what changed on Postgres is not in it"
    );
    clear_marker(data_dir);
    Ok(())
}

fn url_from(url: Option<&str>) -> Result<&str, StoreError> {
    url.ok_or_else(|| {
        StoreError::Database(
            "ISA_DATABASE_URL is not set; it names the Postgres to move to or from".into(),
        )
    })
}

fn command_of(args: &[String]) -> Option<(Direction, bool)> {
    let direction = match args.first()?.as_str() {
        "migrate-to-postgres" => Direction::ToPostgres,
        "migrate-to-sqlite" => Direction::ToSqlite,
        _ => return None,
    };
    Some((direction, args.iter().any(|arg| arg == "--force")))
}

pub fn to_postgres(data_dir: &str, url: &str, force: bool) -> Result<String, StoreError> {
    if !state_db_path(data_dir).exists() {
        return Err(StoreError::Database(format!(
            "there is no state.db in {data_dir} to copy"
        )));
    }
    let db = Db::new(Postgres::connect(url)?);
    let held = db.schema_version()?;
    if held != 0 && !force {
        return Err(StoreError::Database(
            "that Postgres already holds a store; pass --force to replace it with state.db".into(),
        ));
    }
    if held != 0 {
        db.set_schema_version(0)?;
    }
    let old = Store::open(data_dir)?;
    Store::with_db(db, Some(old.raw_db()))?;
    mark_moved(data_dir)?;
    Ok("copied state.db into Postgres; it stays where it is, out of date from now on".into())
}

fn set_aside(data_dir: &str) -> Result<Option<PathBuf>, StoreError> {
    if !state_db_path(data_dir).exists() {
        return Ok(None);
    }
    let folder = Path::new(data_dir).join(format!(
        "state.db.before-{}",
        crate::config::iso_now().replace(':', "-")
    ));
    std::fs::create_dir_all(&folder).map_err(|e| StoreError::Io(e.to_string()))?;
    for name in ["state.db", "state.db-wal", "state.db-shm"] {
        let from = Path::new(data_dir).join(name);
        if from.exists() {
            std::fs::rename(&from, folder.join(name)).map_err(|e| StoreError::Io(e.to_string()))?;
        }
    }
    Ok(Some(folder))
}

pub fn to_sqlite(data_dir: &str, url: &str, force: bool) -> Result<String, StoreError> {
    let db = Db::new(Postgres::connect(url)?);
    if db.schema_version()? == 0 {
        return Err(StoreError::Database("that Postgres holds no store".into()));
    }
    let source = Store::with_db(db, None)?;
    if state_db_path(data_dir).exists() && !has_moved(data_dir) && !force {
        return Err(StoreError::Database(
            "a state.db is already here and no move to Postgres left it; pass --force to replace it (it is set aside first)".into(),
        ));
    }
    let aside = set_aside(data_dir)?;
    let target = Store::open(data_dir)?;
    let copied = import::copy_all(source.raw_db(), target.raw_db())?;
    drop(target);
    clear_marker(data_dir);
    Ok(match aside {
        Some(folder) => format!(
            "copied {copied} row(s) from Postgres into state.db; the old one is in {}",
            folder.display()
        ),
        None => format!("copied {copied} row(s) from Postgres into a new state.db"),
    })
}

pub fn run(args: &[String]) -> Option<i32> {
    let (direction, force) = command_of(args)?;
    let config = crate::config::cfg();
    let outcome = url_from(config.database_url.as_deref()).and_then(|url| match direction {
        Direction::ToPostgres => to_postgres(&config.data_dir, url, force),
        Direction::ToSqlite => to_sqlite(&config.data_dir, url, force),
    });
    match outcome {
        Ok(message) => {
            println!("{message}");
            Some(0)
        }
        Err(e) => {
            eprintln!("{e}");
            Some(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::postgres::Postgres;
    use crate::db::Backend;
    use crate::store::Store;

    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("isa-migrate-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn scoped_url(base: &str, schema: &str) -> (Postgres, String) {
        let admin = Postgres::connect_isolated(base);
        admin
            .script(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema};"
            ))
            .unwrap();
        let url = format!(
            "{base}{}options=-c%20search_path%3D{schema}",
            if base.contains('?') { "&" } else { "?" }
        );
        (admin, url)
    }

    #[test]
    fn a_marker_is_written_found_and_cleared() {
        let dir = scratch_dir("marker");
        let path = dir.to_str().unwrap();
        assert!(!has_moved(path));
        mark_moved(path).unwrap();
        assert!(has_moved(path));
        clear_marker(path);
        assert!(!has_moved(path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sqlite_start_with_no_marker_is_allowed() {
        let dir = scratch_dir("allowed");
        assert!(refuse_stale_sqlite(dir.to_str().unwrap(), false).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_sqlite_start_after_a_move_is_refused_with_the_way_out() {
        let dir = scratch_dir("refused");
        let path = dir.to_str().unwrap();
        mark_moved(path).unwrap();
        let refusal = refuse_stale_sqlite(path, false).unwrap_err().to_string();
        assert!(refusal.contains("ISA_DATABASE_URL"));
        assert!(refusal.contains("migrate-to-sqlite"));
        assert!(refusal.contains("ISA_ALLOW_STALE_STATE_DB"));
        assert!(has_moved(path), "a refusal leaves the marker where it is");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_override_starts_on_the_old_file_and_clears_the_marker() {
        let dir = scratch_dir("override");
        let path = dir.to_str().unwrap();
        mark_moved(path).unwrap();
        assert!(refuse_stale_sqlite(path, true).is_ok());
        assert!(!has_moved(path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_database_url_is_named_not_guessed() {
        assert!(url_from(None)
            .unwrap_err()
            .to_string()
            .contains("ISA_DATABASE_URL"));
        assert_eq!(url_from(Some("postgres://x")).unwrap(), "postgres://x");
    }

    #[test]
    fn only_the_two_commands_are_ours() {
        assert_eq!(
            command_of(&["migrate-to-sqlite".into()]),
            Some((Direction::ToSqlite, false))
        );
        assert_eq!(
            command_of(&["migrate-to-postgres".into(), "--force".into()]),
            Some((Direction::ToPostgres, true))
        );
        assert_eq!(command_of(&[]), None);
        assert_eq!(command_of(&["something-else".into()]), None);
    }

    #[test]
    fn a_round_trip_through_postgres_keeps_everything_and_picks_up_what_changed_there() {
        let Ok(base) = std::env::var("ISA_TEST_POSTGRES_URL") else {
            eprintln!("skipped: ISA_TEST_POSTGRES_URL is not set");
            return;
        };
        let (admin, url) = scoped_url(&base, &format!("isa_roundtrip_{}", std::process::id()));
        let dir = scratch_dir("roundtrip");
        let path = dir.to_str().unwrap();

        let old = Store::open(path).unwrap();
        old.seen_add("m1", "c1", "a1", Some("o1"), false).unwrap();
        old.kv_set("settings", &serde_json::json!({"shareLinkJoin": false}))
            .unwrap();
        drop(old);

        to_postgres(path, &url, false).unwrap();
        assert!(has_moved(path));
        assert!(refuse_stale_sqlite(path, false).is_err());
        assert!(
            to_postgres(path, &url, false).is_err(),
            "a Postgres that already holds a store is not overwritten without --force"
        );

        let on_postgres = Store::open_postgres(path, &url).unwrap();
        on_postgres
            .seen_add("m1", "c2", "a2", Some("o2"), true)
            .unwrap();
        drop(on_postgres);

        to_sqlite(path, &url, false).unwrap();
        assert!(!has_moved(path));
        let back = Store::open(path).unwrap();
        assert_eq!(back.seen_for_mapping("m1").unwrap().len(), 2);
        assert!(back.seen_for_mapping("m1").unwrap()[1].stored_full);
        assert_eq!(
            back.kv("settings").unwrap().unwrap()["shareLinkJoin"],
            false
        );
        drop(back);
        let backups: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("state.db.before-")
            })
            .collect();
        assert_eq!(backups.len(), 1, "the stale file is kept, not deleted");

        assert!(
            to_sqlite(path, &url, false).is_err(),
            "a state.db that no move left behind is not replaced without --force"
        );
        to_sqlite(path, &url, true).unwrap();

        admin
            .script(&format!(
                "DROP SCHEMA isa_roundtrip_{} CASCADE;",
                std::process::id()
            ))
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn moving_back_from_a_database_that_holds_nothing_is_refused() {
        let Ok(base) = std::env::var("ISA_TEST_POSTGRES_URL") else {
            eprintln!("skipped: ISA_TEST_POSTGRES_URL is not set");
            return;
        };
        let (admin, url) = scoped_url(&base, &format!("isa_empty_{}", std::process::id()));
        let dir = scratch_dir("empty");
        let refusal = to_sqlite(dir.to_str().unwrap(), &url, false).unwrap_err();
        assert!(refusal.to_string().contains("holds no store"));
        assert!(!dir.join("state.db").exists());
        admin
            .script(&format!(
                "DROP SCHEMA isa_empty_{} CASCADE;",
                std::process::id()
            ))
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
