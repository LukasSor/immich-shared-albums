/** db/postgres.rs — the Postgres backend: one client on its own worker thread, so the async runtime never blocks inside it. See ARCHITECTURE.md. */
use super::{Backend, Row, Step, Val};
use crate::store::StoreError;
use postgres::types::{to_sql_checked, IsNull, ToSql, Type};
use postgres::{Client, NoTls};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Mutex;
use std::time::Duration;

const CONNECT_ATTEMPTS: u32 = 15;
const CONNECT_WAIT: Duration = Duration::from_secs(2);

enum Work {
    Exec(String, Vec<Val>),
    Rows(String, Vec<Val>),
    Batch(Vec<(String, Vec<Val>)>),
    Script(String),
    ResetSequences(Vec<String>),
}

enum Reply {
    Count(u64),
    Rows(Vec<Row>),
    Done,
}

struct Job {
    work: Work,
    reply: Sender<Result<Reply, String>>,
}

pub struct Postgres {
    jobs: Mutex<Sender<Job>>,
}

impl ToSql for Val {
    fn to_sql(
        &self,
        ty: &Type,
        out: &mut postgres::types::private::BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        match self {
            Val::Null => Ok(IsNull::Yes),
            Val::Text(text) => text.as_str().to_sql(ty, out),
            Val::Int(number) => match *ty {
                Type::INT2 => i16::try_from(*number)?.to_sql(ty, out),
                Type::INT4 => i32::try_from(*number)?.to_sql(ty, out),
                _ => number.to_sql(ty, out),
            },
        }
    }

    fn accepts(_: &Type) -> bool {
        true
    }

    to_sql_checked!();
}

pub fn placeholders(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '?' && chars.peek().is_some_and(char::is_ascii_digit) {
            out.push('$');
        } else {
            out.push(c);
        }
    }
    out
}

pub fn dialect(sql: &str) -> String {
    placeholders(sql)
        .replace("INTEGER PRIMARY KEY", "BIGSERIAL PRIMARY KEY")
        .replace(" INTEGER", " BIGINT")
}

fn decode(row: &postgres::Row) -> Result<Row, String> {
    let mut cells = Vec::with_capacity(row.len());
    for (index, column) in row.columns().iter().enumerate() {
        let cell = match *column.type_() {
            Type::INT8 => row
                .try_get::<_, Option<i64>>(index)
                .map(|v| v.map_or(Val::Null, Val::Int)),
            Type::INT4 => row
                .try_get::<_, Option<i32>>(index)
                .map(|v| v.map_or(Val::Null, |n| Val::Int(i64::from(n)))),
            Type::INT2 => row
                .try_get::<_, Option<i16>>(index)
                .map(|v| v.map_or(Val::Null, |n| Val::Int(i64::from(n)))),
            Type::BOOL => row
                .try_get::<_, Option<bool>>(index)
                .map(|v| v.map_or(Val::Null, |b| Val::Int(i64::from(b)))),
            Type::TEXT | Type::VARCHAR | Type::NAME | Type::BPCHAR => row
                .try_get::<_, Option<String>>(index)
                .map(|v| v.map_or(Val::Null, Val::Text)),
            _ => {
                return Err(format!(
                    "column {} has type {}, which the store does not read",
                    column.name(),
                    column.type_()
                ))
            }
        };
        cells.push(cell.map_err(|e| e.to_string())?);
    }
    Ok(Row(cells))
}

fn refs(params: &[Val]) -> Vec<&(dyn ToSql + Sync)> {
    params.iter().map(|p| p as &(dyn ToSql + Sync)).collect()
}

fn connect(url: &str) -> Result<Client, String> {
    let config: postgres::Config = url
        .parse()
        .map_err(|e: postgres::Error| format!("ISA_DATABASE_URL is not a Postgres URL: {e}"))?;
    let mut last = String::new();
    for attempt in 1..=CONNECT_ATTEMPTS {
        match config.connect(NoTls) {
            Ok(client) => return Ok(client),
            Err(e) => {
                last = e.to_string();
                if attempt < CONNECT_ATTEMPTS {
                    crate::log!(
                        "the database is not reachable yet (attempt {attempt}): {last} — retrying"
                    );
                    std::thread::sleep(CONNECT_WAIT);
                }
            }
        }
    }
    Err(last)
}

fn run(client: &mut Client, work: &Work) -> Result<Reply, String> {
    let failed = |e: postgres::Error| e.to_string();
    match work {
        Work::Exec(sql, params) => Ok(Reply::Count(
            client
                .execute(sql.as_str(), &refs(params))
                .map_err(failed)?,
        )),
        Work::Rows(sql, params) => {
            let rows = client.query(sql.as_str(), &refs(params)).map_err(failed)?;
            Ok(Reply::Rows(
                rows.iter()
                    .map(decode)
                    .collect::<Result<Vec<Row>, String>>()?,
            ))
        }
        Work::Batch(steps) => {
            let mut tx = client.transaction().map_err(failed)?;
            for (sql, params) in steps {
                tx.execute(sql.as_str(), &refs(params)).map_err(failed)?;
            }
            tx.commit().map_err(failed)?;
            Ok(Reply::Done)
        }
        Work::Script(sql) => {
            client.batch_execute(sql).map_err(failed)?;
            Ok(Reply::Done)
        }
        Work::ResetSequences(tables) => {
            for table in tables {
                client
                    .batch_execute(&format!(
                        "SELECT setval(pg_get_serial_sequence('{table}', 'id'), \
                         COALESCE((SELECT MAX(id) FROM {table}), 1), \
                         (SELECT MAX(id) FROM {table}) IS NOT NULL)"
                    ))
                    .map_err(failed)?;
            }
            Ok(Reply::Done)
        }
    }
}

fn serve(url: String, mut client: Client, jobs: Receiver<Job>) {
    while let Ok(job) = jobs.recv() {
        if client.is_closed() {
            match connect(&url) {
                Ok(fresh) => client = fresh,
                Err(e) => {
                    let _ = job
                        .reply
                        .send(Err(format!("the database connection is lost: {e}")));
                    continue;
                }
            }
        }
        let _ = job.reply.send(run(&mut client, &job.work));
    }
}

impl Postgres {
    pub fn connect(url: &str) -> Result<Postgres, StoreError> {
        let client = connect(url).map_err(StoreError::Database)?;
        let (sender, receiver) = channel();
        let owned = url.to_string();
        std::thread::Builder::new()
            .name("isa-postgres".into())
            .spawn(move || serve(owned, client, receiver))
            .map_err(|e| StoreError::Io(e.to_string()))?;
        Ok(Postgres {
            jobs: Mutex::new(sender),
        })
    }

    fn ask(&self, work: Work) -> Result<Reply, StoreError> {
        let (reply, answer) = channel();
        self.jobs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .send(Job { work, reply })
            .map_err(|_| StoreError::Database("the database worker has stopped".into()))?;
        answer
            .recv()
            .map_err(|_| StoreError::Database("the database worker has stopped".into()))?
            .map_err(StoreError::Database)
    }
}

#[cfg(test)]
impl Postgres {
    pub fn connect_isolated(url: &str) -> Postgres {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let schema = format!(
            "isa_test_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        );
        let postgres = Postgres::connect(url).expect("the test Postgres is reachable");
        postgres
            .script(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}; SET search_path TO {schema};"
            ))
            .expect("an isolated schema");
        postgres
    }
}

impl Backend for Postgres {
    fn exec(&self, sql: &str, params: &[Val]) -> Result<u64, StoreError> {
        match self.ask(Work::Exec(dialect(sql), params.to_vec()))? {
            Reply::Count(count) => Ok(count),
            _ => Ok(0),
        }
    }

    fn rows(&self, sql: &str, params: &[Val]) -> Result<Vec<Row>, StoreError> {
        match self.ask(Work::Rows(dialect(sql), params.to_vec()))? {
            Reply::Rows(rows) => Ok(rows),
            _ => Ok(Vec::new()),
        }
    }

    fn batch(&self, steps: &[Step]) -> Result<(), StoreError> {
        let translated = steps
            .iter()
            .map(|step| (dialect(step.sql), step.params.clone()))
            .collect();
        self.ask(Work::Batch(translated)).map(|_| ())
    }

    fn script(&self, sql: &str) -> Result<(), StoreError> {
        self.ask(Work::Script(dialect(sql))).map(|_| ())
    }

    fn schema_version(&self) -> Result<i64, StoreError> {
        self.script("CREATE TABLE IF NOT EXISTS schema_info (id INTEGER PRIMARY KEY, version INTEGER NOT NULL);")?;
        let rows = self.rows("SELECT version FROM schema_info WHERE id = 1", &[])?;
        rows.first().map_or(Ok(0), |row| row.int(0))
    }

    fn set_schema_version(&self, version: i64) -> Result<(), StoreError> {
        self.exec(
            "INSERT INTO schema_info (id, version) VALUES (1, ?1)
             ON CONFLICT (id) DO UPDATE SET version = excluded.version",
            &[Val::Int(version)],
        )
        .map(|_| ())
    }

    fn add_column_if_missing(
        &self,
        table: &str,
        column: &str,
        definition: &str,
    ) -> Result<(), StoreError> {
        self.script(&format!(
            "ALTER TABLE {table} ADD COLUMN IF NOT EXISTS {column} {definition};"
        ))
    }

    fn reset_id_sequences(&self, tables: &[&str]) -> Result<(), StoreError> {
        let names = tables.iter().map(|t| t.to_string()).collect();
        self.ask(Work::ResetSequences(names)).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbered_placeholders_become_dollar_placeholders() {
        assert_eq!(
            placeholders("SELECT a FROM t WHERE x = ?1 AND y = ?12"),
            "SELECT a FROM t WHERE x = $1 AND y = $12"
        );
    }

    #[test]
    fn a_question_mark_that_is_not_a_placeholder_is_left_alone() {
        assert_eq!(placeholders("SELECT '?'"), "SELECT '?'");
    }

    #[test]
    fn integers_widen_and_autoincrement_keys_become_serials() {
        let ddl = dialect("CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER NOT NULL DEFAULT 0);");
        assert_eq!(
            ddl,
            "CREATE TABLE t (id BIGSERIAL PRIMARY KEY, n BIGINT NOT NULL DEFAULT 0);"
        );
    }
}
