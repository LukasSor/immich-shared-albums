/** db/mod.rs — the one SQL surface the store talks to; SQLite and Postgres each implement it. See ARCHITECTURE.md. */
pub mod import;
pub mod postgres;
pub mod sqlite;

use crate::store::StoreError;
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Null,
    Int(i64),
    Text(String),
}

impl From<&str> for Val {
    fn from(text: &str) -> Val {
        Val::Text(text.to_string())
    }
}

impl From<&String> for Val {
    fn from(text: &String) -> Val {
        Val::Text(text.clone())
    }
}

impl From<String> for Val {
    fn from(text: String) -> Val {
        Val::Text(text)
    }
}

impl From<i64> for Val {
    fn from(number: i64) -> Val {
        Val::Int(number)
    }
}

impl From<i32> for Val {
    fn from(number: i32) -> Val {
        Val::Int(i64::from(number))
    }
}

impl From<usize> for Val {
    fn from(number: usize) -> Val {
        Val::Int(number as i64)
    }
}

impl From<bool> for Val {
    fn from(flag: bool) -> Val {
        Val::Int(i64::from(flag))
    }
}

impl<T: Into<Val>> From<Option<T>> for Val {
    fn from(value: Option<T>) -> Val {
        value.map_or(Val::Null, Into::into)
    }
}

#[macro_export]
macro_rules! params {
    ($($value:expr),* $(,)?) => {
        &[$($crate::db::Val::from($value)),*][..]
    };
}

#[derive(Debug)]
pub struct Row(pub Vec<Val>);

impl Row {
    fn cell(&self, index: usize) -> Result<&Val, StoreError> {
        self.0
            .get(index)
            .ok_or_else(|| StoreError::Database(format!("column {index} is missing from a row")))
    }

    fn wrong_type(index: usize, wanted: &str) -> StoreError {
        StoreError::Database(format!("column {index} is not {wanted}"))
    }

    pub fn int(&self, index: usize) -> Result<i64, StoreError> {
        match self.cell(index)? {
            Val::Int(number) => Ok(*number),
            _ => Err(Self::wrong_type(index, "an integer")),
        }
    }

    pub fn opt_int(&self, index: usize) -> Result<Option<i64>, StoreError> {
        match self.cell(index)? {
            Val::Null => Ok(None),
            Val::Int(number) => Ok(Some(*number)),
            Val::Text(_) => Err(Self::wrong_type(index, "an integer")),
        }
    }

    pub fn text(&self, index: usize) -> Result<String, StoreError> {
        match self.cell(index)? {
            Val::Text(text) => Ok(text.clone()),
            _ => Err(Self::wrong_type(index, "text")),
        }
    }

    pub fn opt_text(&self, index: usize) -> Result<Option<String>, StoreError> {
        match self.cell(index)? {
            Val::Null => Ok(None),
            Val::Text(text) => Ok(Some(text.clone())),
            Val::Int(_) => Err(Self::wrong_type(index, "text")),
        }
    }

    pub fn flag(&self, index: usize) -> Result<bool, StoreError> {
        Ok(self.int(index)? != 0)
    }

    pub fn opt_flag(&self, index: usize) -> Result<Option<bool>, StoreError> {
        Ok(self.opt_int(index)?.map(|number| number != 0))
    }
}

pub struct Step<'a> {
    pub sql: &'a str,
    pub params: Vec<Val>,
}

pub trait Backend: Send + Sync {
    fn exec(&self, sql: &str, params: &[Val]) -> Result<u64, StoreError>;
    fn rows(&self, sql: &str, params: &[Val]) -> Result<Vec<Row>, StoreError>;
    fn batch(&self, steps: &[Step]) -> Result<(), StoreError>;
    fn script(&self, sql: &str) -> Result<(), StoreError>;
    fn schema_version(&self) -> Result<i64, StoreError>;
    fn set_schema_version(&self, version: i64) -> Result<(), StoreError>;
    fn add_column_if_missing(
        &self,
        table: &str,
        column: &str,
        definition: &str,
    ) -> Result<(), StoreError>;
    fn reset_id_sequences(&self, _tables: &[&str]) -> Result<(), StoreError> {
        Ok(())
    }
}

pub struct Db(Box<dyn Backend>);

impl Db {
    pub fn new(backend: impl Backend + 'static) -> Db {
        Db(Box::new(backend))
    }

    pub fn exec(&self, sql: &str, params: &[Val]) -> Result<u64, StoreError> {
        self.0.exec(sql, params)
    }

    pub fn rows(&self, sql: &str, params: &[Val]) -> Result<Vec<Row>, StoreError> {
        self.0.rows(sql, params)
    }

    pub fn first(&self, sql: &str, params: &[Val]) -> Result<Option<Row>, StoreError> {
        Ok(self.0.rows(sql, params)?.into_iter().next())
    }

    pub fn count(&self, sql: &str, params: &[Val]) -> Result<i64, StoreError> {
        self.first(sql, params)?.map_or(Ok(0), |row| row.int(0))
    }

    pub fn batch(&self, steps: &[Step]) -> Result<(), StoreError> {
        self.0.batch(steps)
    }

    pub fn script(&self, sql: &str) -> Result<(), StoreError> {
        self.0.script(sql)
    }

    pub fn schema_version(&self) -> Result<i64, StoreError> {
        self.0.schema_version()
    }

    pub fn set_schema_version(&self, version: i64) -> Result<(), StoreError> {
        self.0.set_schema_version(version)
    }

    pub fn add_column_if_missing(
        &self,
        table: &str,
        column: &str,
        definition: &str,
    ) -> Result<(), StoreError> {
        self.0.add_column_if_missing(table, column, definition)
    }

    pub fn reset_id_sequences(&self, tables: &[&str]) -> Result<(), StoreError> {
        self.0.reset_id_sequences(tables)
    }
}

/// Take a non-reentrant lock, refusing to spin forever: once the wait outlives `SAVE_LOCK_WAIT`
/// the failure is NAMED (a self-deadlock or severe contention — the deadlock class this port has
/// hit repeatedly), and the ordinary blocking lock is taken as the log says, so the caller either
/// proceeds or blocks visibly instead of spinning silently for ever. Poison is recovered because a
/// poisoned lock here means a task panicked mid-save; the last committed data still stands and
/// refusing it would lose every later write.
pub(crate) fn bounded_lock<'a, T>(
    mutex: &'a Mutex<T>,
    held: &str,
    waiting_for: &str,
) -> std::sync::MutexGuard<'a, T> {
    const SAVE_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
    const RETRY_SLEEP: std::time::Duration = std::time::Duration::from_millis(5);
    let deadline = std::time::Instant::now() + SAVE_LOCK_WAIT;
    loop {
        match mutex.try_lock() {
            Ok(guard) => return guard,
            Err(std::sync::TryLockError::Poisoned(e)) => return e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    crate::log!(
                        "SAVE blocked {SAVE_LOCK_WAIT:?} waiting for {waiting_for} while holding {held} — every other task is blocked behind this one. Blocking as normal, but this is the bug."
                    );
                    return mutex.lock().unwrap_or_else(|e| e.into_inner());
                }
                std::thread::sleep(RETRY_SLEEP);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_convert_from_the_types_the_store_binds() {
        assert_eq!(Val::from("a"), Val::Text("a".into()));
        assert_eq!(Val::from(7usize), Val::Int(7));
        assert_eq!(Val::from(true), Val::Int(1));
        assert_eq!(Val::from(None::<String>), Val::Null);
        assert_eq!(Val::from(Some("b")), Val::Text("b".into()));
    }

    #[test]
    fn a_row_reads_each_cell_as_the_type_it_holds() {
        let row = Row(vec![
            Val::Int(3),
            Val::Text("x".into()),
            Val::Null,
            Val::Int(0),
        ]);
        assert_eq!(row.int(0).unwrap(), 3);
        assert_eq!(row.text(1).unwrap(), "x");
        assert_eq!(row.opt_text(2).unwrap(), None);
        assert_eq!(row.opt_int(2).unwrap(), None);
        assert!(!row.flag(3).unwrap());
        assert_eq!(row.opt_flag(2).unwrap(), None);
    }

    #[test]
    fn a_cell_of_the_wrong_type_or_position_is_an_error_not_a_default() {
        let row = Row(vec![Val::Text("x".into())]);
        assert!(row.int(0).is_err());
        assert!(row.text(1).is_err());
        assert!(Row(vec![Val::Null]).text(0).is_err());
    }
}
