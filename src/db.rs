use rusqlite::Connection;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS requests (
  id                INTEGER PRIMARY KEY,
  ts                INTEGER NOT NULL,
  endpoint          TEXT    NOT NULL,
  model             TEXT    NOT NULL,
  prompt_tokens     INTEGER NOT NULL,
  completion_tokens INTEGER NOT NULL,
  reasoning_tokens  INTEGER NOT NULL,
  total_tokens      INTEGER NOT NULL,
  stream            INTEGER NOT NULL,
  status            INTEGER NOT NULL,
  duration_ms       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS requests_ts ON requests(ts);
";

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn open_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}
