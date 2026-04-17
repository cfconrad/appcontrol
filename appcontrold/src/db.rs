use rusqlite::{Connection, Result};

use crate::proc::ProcSnapshot;

pub struct ActiveProcess {
    pub db_id: i64,
    pub start_epoch: i64,
}

pub fn open_db(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS boot_sessions (
             id        INTEGER PRIMARY KEY AUTOINCREMENT,
             boot_time INTEGER NOT NULL UNIQUE
         );
         CREATE TABLE IF NOT EXISTS processes (
             id               INTEGER PRIMARY KEY AUTOINCREMENT,
             boot_id          INTEGER NOT NULL REFERENCES boot_sessions(id),
             pid              INTEGER NOT NULL,
             uid              INTEGER NOT NULL DEFAULT 0,
             name             TEXT    NOT NULL,
             cmdline          TEXT    NOT NULL,
             start_time       INTEGER NOT NULL,
             end_time         INTEGER,
             duration_seconds INTEGER
         );
         CREATE INDEX IF NOT EXISTS idx_processes_boot_pid
             ON processes(boot_id, pid, start_time);",
    )?;
    // Migrate existing databases that predate the uid column.
    let _ = conn.execute(
        "ALTER TABLE processes ADD COLUMN uid INTEGER NOT NULL DEFAULT 0",
        [],
    );
    Ok(conn)
}

pub fn get_or_create_boot_session(conn: &Connection, boot_time: i64) -> Result<i64> {
    conn.execute(
        "INSERT OR IGNORE INTO boot_sessions (boot_time) VALUES (?1)",
        [boot_time],
    )?;
    conn.query_row(
        "SELECT id FROM boot_sessions WHERE boot_time = ?1",
        [boot_time],
        |row| row.get(0),
    )
}

pub fn insert_process(conn: &Connection, boot_id: i64, snap: &ProcSnapshot) -> Result<i64> {
    conn.execute(
        "INSERT INTO processes (boot_id, pid, uid, name, cmdline, start_time)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            boot_id,
            snap.pid,
            snap.uid,
            snap.name,
            snap.cmdline,
            snap.start_epoch
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub struct ProcessRecord {
    pub id: i64,
    pub pid: u32,
    pub name: String,
    pub cmdline: String,
    pub start_time: i64,
}

pub fn list_active_processes(conn: &Connection) -> Result<Vec<ProcessRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, pid, name, cmdline, start_time
         FROM processes
         WHERE end_time IS NULL
         ORDER BY start_time",
    )?;
    let records = stmt
        .query_map([], |row| {
            Ok(ProcessRecord {
                id: row.get(0)?,
                pid: row.get(1)?,
                name: row.get(2)?,
                cmdline: row.get(3)?,
                start_time: row.get(4)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(records)
}

pub struct TodayRecord {
    pub name: String,
    pub start_time: i64,
    pub end_time: Option<i64>,
}

pub fn list_processes_active_today(
    conn: &Connection,
    today_start: i64,
    now: i64,
) -> Result<Vec<TodayRecord>> {
    let mut stmt = conn.prepare(
        "SELECT name, start_time, end_time
         FROM processes
         WHERE start_time < ?2
           AND (end_time IS NULL OR end_time >= ?1)
         ORDER BY name, start_time",
    )?;
    let records = stmt
        .query_map(rusqlite::params![today_start, now], |row| {
            Ok(TodayRecord {
                name: row.get(0)?,
                start_time: row.get(1)?,
                end_time: row.get(2)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(records)
}

pub struct EntryRecord {
    pub id: i64,
    pub pid: u32,
    pub uid: u32,
    pub cmdline: String,
    pub start_time: i64,
    pub end_time: Option<i64>,
    pub duration_seconds: Option<i64>,
}

/// `since` is an optional Unix timestamp; when set, only entries with
/// `start_time >= since` are returned.
pub fn list_entries_by_name(
    conn: &Connection,
    name: &str,
    since: Option<i64>,
) -> Result<Vec<EntryRecord>> {
    let cutoff = since.unwrap_or(0);
    let mut stmt = conn.prepare(
        "SELECT id, pid, uid, cmdline, start_time, end_time, duration_seconds
         FROM processes
         WHERE name = ?1
           AND start_time >= ?2
         ORDER BY start_time DESC",
    )?;
    let records = stmt
        .query_map(rusqlite::params![name, cutoff], |row| {
            Ok(EntryRecord {
                id: row.get(0)?,
                pid: row.get(1)?,
                uid: row.get(2)?,
                cmdline: row.get(3)?,
                start_time: row.get(4)?,
                end_time: row.get(5)?,
                duration_seconds: row.get(6)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(records)
}

pub fn finalize_process(conn: &Connection, db_id: i64, end_time: i64, duration: i64) -> Result<()> {
    conn.execute(
        "UPDATE processes SET end_time = ?1, duration_seconds = ?2 WHERE id = ?3",
        rusqlite::params![end_time, duration, db_id],
    )?;
    Ok(())
}
