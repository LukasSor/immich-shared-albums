/** db/sqlite.rs — the SQLite backend: one connection behind a mutex, WAL, and `user_version` as the schema stamp. See ARCHITECTURE.md. */
use super::{bounded_lock, Backend, Row, Step, Val};
use crate::store::StoreError;
use rusqlite::types::{Value, ValueRef};
use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;

pub struct Sqlite {
    conn: Mutex<Connection>,
}

fn bound(params: &[Val]) -> Vec<Value> {
    params
        .iter()
        .map(|param| match param {
            Val::Null => Value::Null,
            Val::Int(number) => Value::Integer(*number),
            Val::Text(text) => Value::Text(text.clone()),
        })
        .collect()
}

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    let width = row.as_ref().column_count();
    let mut cells = Vec::with_capacity(width);
    for index in 0..width {
        cells.push(match row.get_ref(index)? {
            ValueRef::Null => Val::Null,
            ValueRef::Integer(number) => Val::Int(number),
            ValueRef::Text(bytes) => Val::Text(String::from_utf8_lossy(bytes).into_owned()),
            ValueRef::Real(_) | ValueRef::Blob(_) => {
                return Err(rusqlite::Error::InvalidColumnType(
                    index,
                    "a column".into(),
                    rusqlite::types::Type::Blob,
                ))
            }
        });
    }
    Ok(Row(cells))
}

impl Sqlite {
    pub fn open(data_dir: &str) -> Result<Sqlite, StoreError> {
        Self::from_connection(Connection::open(Path::new(data_dir).join("state.db"))?)
    }

    pub fn in_memory() -> Result<Sqlite, StoreError> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> Result<Sqlite, StoreError> {
        // The busy timeout is installed BEFORE the WAL pragma below: the pragma takes a write
        // lock, and two concurrent boots — a restart overlapping a boot, a test booting the same
        // directory — used to fail HERE with `database is locked`, before any timeout existed to
        // make them wait. 5s, so a genuinely stuck lock still surfaces rather than hanging.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode = WAL;")?;
        Ok(Sqlite {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Backend for Sqlite {
    fn exec(&self, sql: &str, params: &[Val]) -> Result<u64, StoreError> {
        let changed = self
            .lock()
            .execute(sql, rusqlite::params_from_iter(bound(params)))?;
        Ok(changed as u64)
    }

    fn rows(&self, sql: &str, params: &[Val]) -> Result<Vec<Row>, StoreError> {
        let conn = self.lock();
        let mut statement = conn.prepare(sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(bound(params)), read_row)?;
        Ok(rows.collect::<Result<Vec<Row>, _>>()?)
    }

    fn batch(&self, steps: &[Step]) -> Result<(), StoreError> {
        let mut conn = bounded_lock(&self.conn, "the state lock", "the database");
        let tx = conn.transaction()?;
        for step in steps {
            tx.execute(step.sql, rusqlite::params_from_iter(bound(&step.params)))?;
        }
        tx.commit()?;
        Ok(())
    }

    fn script(&self, sql: &str) -> Result<(), StoreError> {
        Ok(self.lock().execute_batch(sql)?)
    }

    fn schema_version(&self) -> Result<i64, StoreError> {
        Ok(self
            .lock()
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    fn set_schema_version(&self, version: i64) -> Result<(), StoreError> {
        Ok(self
            .lock()
            .execute_batch(&format!("PRAGMA user_version = {version};"))?)
    }

    fn add_column_if_missing(
        &self,
        table: &str,
        column: &str,
        definition: &str,
    ) -> Result<(), StoreError> {
        let conn = self.lock();
        let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let existing: Vec<String> = statement
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .collect();
        drop(statement);
        if !existing.iter().any(|name| name == column) {
            conn.execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {column} {definition};"
            ))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params;

    fn db() -> Sqlite {
        let sqlite = Sqlite::in_memory().unwrap();
        sqlite
            .script("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, n INTEGER);")
            .unwrap();
        sqlite
    }

    #[test]
    fn rows_come_back_typed_including_nulls() {
        let sqlite = db();
        sqlite
            .exec("INSERT INTO t (name, n) VALUES (?1, ?2)", params!["a", 5])
            .unwrap();
        sqlite
            .exec(
                "INSERT INTO t (name, n) VALUES (?1, ?2)",
                params![None::<String>, None::<i64>],
            )
            .unwrap();
        let rows = sqlite
            .rows("SELECT name, n FROM t ORDER BY id", &[])
            .unwrap();
        assert_eq!(rows[0].text(0).unwrap(), "a");
        assert_eq!(rows[0].int(1).unwrap(), 5);
        assert_eq!(rows[1].opt_text(0).unwrap(), None);
        assert_eq!(rows[1].opt_int(1).unwrap(), None);
    }

    #[test]
    fn a_batch_is_all_or_nothing() {
        let sqlite = db();
        let outcome = sqlite.batch(&[
            Step {
                sql: "INSERT INTO t (name) VALUES (?1)",
                params: vec![Val::from("kept?")],
            },
            Step {
                sql: "INSERT INTO nowhere VALUES (1)",
                params: vec![],
            },
        ]);
        assert!(outcome.is_err());
        assert!(sqlite.rows("SELECT id FROM t", &[]).unwrap().is_empty());
    }

    #[test]
    fn the_schema_version_round_trips_and_a_column_is_added_once() {
        let sqlite = db();
        assert_eq!(sqlite.schema_version().unwrap(), 0);
        sqlite.set_schema_version(5).unwrap();
        assert_eq!(sqlite.schema_version().unwrap(), 5);
        sqlite.add_column_if_missing("t", "extra", "TEXT").unwrap();
        sqlite.add_column_if_missing("t", "extra", "TEXT").unwrap();
        sqlite
            .exec("INSERT INTO t (extra) VALUES (?1)", params!["x"])
            .unwrap();
    }

    #[test]
    fn an_insert_can_hand_back_its_new_id() {
        let sqlite = db();
        let rows = sqlite
            .rows(
                "INSERT INTO t (name) VALUES (?1) RETURNING id",
                params!["a"],
            )
            .unwrap();
        assert_eq!(rows[0].int(0).unwrap(), 1);
    }
}
