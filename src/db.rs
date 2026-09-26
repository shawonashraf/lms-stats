use rusqlite::{Connection, params};
use serde::Serialize;

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

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub id: i64,
    /// Unix seconds at request start.
    pub ts: i64,
    pub endpoint: String,
    pub model: String,
    pub prompt_tokens: i64,
    /// As reported by LM Studio; already includes `reasoning_tokens`.
    pub completion_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_tokens: i64,
    pub stream: bool,
    pub status: u16,
    pub duration_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Totals {
    pub requests: i64,
    pub prompt: i64,
    pub completion: i64,
    pub reasoning: i64,
    pub total: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bucket {
    pub key: String,
    #[serde(flatten)]
    pub totals: Totals,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BucketSize {
    Day,
    Week,
    Month,
    Year,
}

impl BucketSize {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "day" => Some(Self::Day),
            "week" => Some(Self::Week),
            "month" => Some(Self::Month),
            "year" => Some(Self::Year),
            _ => None,
        }
    }

    /// Full SQL expression producing this bucket's key from `ts`.
    fn sql_key(self) -> &'static str {
        match self {
            Self::Day => "strftime('%Y-%m-%d', ts, 'unixepoch', 'localtime')",
            // Monday of the week containing `ts`: advance to the next Sunday
            // (or stay if already Sunday), then step back 6 days.
            Self::Week => "date(ts, 'unixepoch', 'localtime', 'weekday 0', '-6 days')",
            Self::Month => "strftime('%Y-%m', ts, 'unixepoch', 'localtime')",
            Self::Year => "strftime('%Y', ts, 'unixepoch', 'localtime')",
        }
    }
}

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn open_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

pub fn insert(conn: &Connection, r: &Row) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO requests (ts, endpoint, model, prompt_tokens, completion_tokens, reasoning_tokens,
                               total_tokens, stream, status, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            r.ts,
            r.endpoint,
            r.model,
            r.prompt_tokens,
            r.completion_tokens,
            r.reasoning_tokens,
            r.total_tokens,
            r.stream as i64,
            r.status as i64,
            r.duration_ms
        ],
    )?;
    Ok(())
}

pub fn list(conn: &Connection, limit: i64, offset: i64, model: Option<&str>) -> rusqlite::Result<Vec<Row>> {
    let mut stmt = conn.prepare(
        "SELECT id, ts, endpoint, model, prompt_tokens, completion_tokens, reasoning_tokens,
                total_tokens, stream, status, duration_ms
         FROM requests
         WHERE (?1 IS NULL OR model = ?1)
         ORDER BY id DESC
         LIMIT ?2 OFFSET ?3",
    )?;
    let rows = stmt.query_map(params![model, limit, offset], |r| {
        Ok(Row {
            id: r.get(0)?,
            ts: r.get(1)?,
            endpoint: r.get(2)?,
            model: r.get(3)?,
            prompt_tokens: r.get(4)?,
            completion_tokens: r.get(5)?,
            reasoning_tokens: r.get(6)?,
            total_tokens: r.get(7)?,
            stream: r.get::<_, i64>(8)? != 0,
            status: r.get::<_, i64>(9)? as u16,
            duration_ms: r.get(10)?,
        })
    })?;
    rows.collect()
}

pub fn aggregate(
    conn: &Connection,
    bucket: BucketSize,
    from: i64,
    to: i64,
    model: Option<&str>,
) -> rusqlite::Result<(Totals, Vec<Bucket>)> {
    let sql = format!(
        "SELECT {} AS k,
                COUNT(*), SUM(prompt_tokens), SUM(completion_tokens), SUM(reasoning_tokens), SUM(total_tokens)
         FROM requests
         WHERE ts >= ?1 AND ts < ?2 AND (?3 IS NULL OR model = ?3)
         GROUP BY k ORDER BY k",
        bucket.sql_key()
    );
    let mut stmt = conn.prepare(&sql)?;
    let buckets = stmt
        .query_map(params![from, to, model], |r| {
            Ok(Bucket {
                key: r.get(0)?,
                totals: Totals {
                    requests: r.get(1)?,
                    prompt: r.get(2)?,
                    completion: r.get(3)?,
                    reasoning: r.get(4)?,
                    total: r.get(5)?,
                },
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut totals = Totals::default();
    for b in &buckets {
        totals.requests += b.totals.requests;
        totals.prompt += b.totals.prompt;
        totals.completion += b.totals.completion;
        totals.reasoning += b.totals.reasoning;
        totals.total += b.totals.total;
    }
    Ok((totals, buckets))
}

pub fn models(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT DISTINCT model FROM requests WHERE model <> '' ORDER BY model")?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-09-25 12:00 UTC and 2026-09-26 12:00 UTC. Noon UTC keeps the calendar
    // day stable under any real timezone offset (bucketing uses server localtime).
    const DAY1: i64 = 1_790_337_600;
    const DAY2: i64 = 1_790_424_000;

    fn row(ts: i64, model: &str, prompt: i64, completion: i64, reasoning: i64) -> Row {
        Row {
            id: 0,
            ts,
            endpoint: "/v1/chat/completions".into(),
            model: model.into(),
            prompt_tokens: prompt,
            completion_tokens: completion,
            reasoning_tokens: reasoning,
            total_tokens: prompt + completion,
            stream: false,
            status: 200,
            duration_ms: 10,
        }
    }

    fn seeded() -> Connection {
        let c = open_memory().unwrap();
        insert(&c, &row(DAY1, "a", 10, 5, 2)).unwrap();
        insert(&c, &row(DAY1 + 60, "b", 20, 8, 0)).unwrap();
        insert(&c, &row(DAY2, "a", 30, 12, 4)).unwrap();
        c
    }

    #[test]
    fn list_is_newest_first_and_filters_by_model() {
        let c = seeded();
        let all = list(&c, 50, 0, None).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].ts, DAY2);
        assert_eq!(all[2].ts, DAY1);
        assert_eq!(all[0].status, 200);

        let only_a = list(&c, 50, 0, Some("a")).unwrap();
        assert_eq!(only_a.len(), 2);
        assert!(only_a.iter().all(|r| r.model == "a"));

        let page = list(&c, 1, 1, None).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].ts, DAY1 + 60);
    }

    #[test]
    fn aggregate_by_day_week_month_year() {
        let c = seeded();
        let (totals, days) = aggregate(&c, BucketSize::Day, 0, i64::MAX, None).unwrap();
        assert_eq!(days.len(), 2);
        assert_eq!(days[0].totals, Totals { requests: 2, prompt: 30, completion: 13, reasoning: 2, total: 43 });
        assert_eq!(days[1].totals, Totals { requests: 1, prompt: 30, completion: 12, reasoning: 4, total: 42 });
        assert_eq!(totals, Totals { requests: 3, prompt: 60, completion: 25, reasoning: 6, total: 85 });

        // Fri 25 and Sat 26 Sep 2026 share a Monday-start week; both timestamps
        // are noon UTC so the Monday key holds in any real timezone.
        let (_, weeks) = aggregate(&c, BucketSize::Week, 0, i64::MAX, None).unwrap();
        assert_eq!(weeks.len(), 1);
        assert_eq!(weeks[0].key, "2026-09-21");
        let (_, months) = aggregate(&c, BucketSize::Month, 0, i64::MAX, None).unwrap();
        assert_eq!(months.len(), 1);
        let (_, years) = aggregate(&c, BucketSize::Year, 0, i64::MAX, None).unwrap();
        assert_eq!(years.len(), 1);
        assert_eq!(years[0].key, "2026");
    }

    #[test]
    fn aggregate_respects_range_and_model() {
        let c = seeded();
        let (totals, days) = aggregate(&c, BucketSize::Day, DAY2, i64::MAX, None).unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!(totals.requests, 1);

        let (totals, _) = aggregate(&c, BucketSize::Day, 0, i64::MAX, Some("b")).unwrap();
        assert_eq!(totals, Totals { requests: 1, prompt: 20, completion: 8, reasoning: 0, total: 28 });
    }

    #[test]
    fn models_are_distinct_and_sorted() {
        let c = seeded();
        insert(&c, &row(DAY1, "", 1, 1, 0)).unwrap();
        let names = models(&c).unwrap();
        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
        assert!(!names.contains(&String::new()));
    }

    #[test]
    fn bucket_size_parses() {
        assert!(matches!(BucketSize::parse("day"), Some(BucketSize::Day)));
        assert!(matches!(BucketSize::parse("year"), Some(BucketSize::Year)));
        assert!(BucketSize::parse("hour").is_none());
    }
}
