use std::collections::HashMap;

use rusqlite::{params, Connection, Result};

/// Open (or create) the progress database at `path`.
pub fn open_progress_db(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS word_progress (
            username TEXT    NOT NULL,
            word     TEXT    NOT NULL,
            correct  INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (username, word)
        );",
    )?;
    Ok(conn)
}

/// Load all (word → correct_count) pairs for `username`.
/// Words not yet in the DB are simply absent (treat as 0).
pub fn load_progress(conn: &Connection, username: &str) -> HashMap<String, u32> {
    let mut stmt = match conn
        .prepare("SELECT word, correct FROM word_progress WHERE username = ?1")
    {
        Ok(s) => s,
        Err(_) => return HashMap::new(),
    };
    stmt.query_map(params![username], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
    })
    .map(|rows| rows.filter_map(|r| r.ok()).collect())
    .unwrap_or_default()
}

/// Increment the correct-answer count for `word` by 1 (inserting if absent).
pub fn increment_correct(conn: &Connection, username: &str, word: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO word_progress (username, word, correct) VALUES (?1, ?2, 1)
         ON CONFLICT(username, word) DO UPDATE SET correct = correct + 1",
        params![username, word],
    )?;
    Ok(())
}

/// Return all `(word, correct)` pairs for `username` sorted by correct ASC, word ASC.
pub fn list_progress(conn: &Connection, username: &str) -> Result<Vec<(String, u32)>> {
    let mut stmt = conn.prepare(
        "SELECT word, correct FROM word_progress WHERE username = ?1
         ORDER BY correct ASC, word ASC",
    )?;
    let rows = stmt
        .query_map(params![username], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Set the correct count for a word to an exact value (used by the edit command).
pub fn set_correct(conn: &Connection, username: &str, word: &str, correct: u32) -> Result<()> {
    conn.execute(
        "INSERT INTO word_progress (username, word, correct) VALUES (?1, ?2, ?3)
         ON CONFLICT(username, word) DO UPDATE SET correct = ?3",
        params![username, word, correct],
    )?;
    Ok(())
}
