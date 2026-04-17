use regex::Regex;
use rusqlite::{Connection, Result};

pub struct Whitelist {
    pub entries: Vec<(Option<u32>, Regex)>,
}

impl Whitelist {
    pub fn matches(&self, uid: u32, name: &str) -> bool {
        self.entries
            .iter()
            .any(|(entry_uid, re)| entry_uid.map_or(true, |u| u == uid) && re.is_match(name))
    }
}

pub struct WhitelistEntry {
    pub id: i64,
    pub uid: Option<u32>,
    pub group_name: String,
    pub pattern: String,
    pub enabled: bool,
}

pub struct WhitelistPattern {
    pub uid: Option<u32>,
    pub group_name: String,
    pub pattern: String,
}

pub struct Rule {
    pub id: i64,
    pub group_name: String,
    pub reset_behavior: String,
    pub limit: i64, // minutes
}

/// A per-group vocabulary quiz rule: earn extra screen time by answering correctly.
#[derive(Clone)]
pub struct VocabRule {
    pub id: i64,
    pub group_name: String,
    /// How many correct answers are needed to earn extra time.
    pub correct_needed: i64,
    /// Extra minutes awarded when the threshold is reached.
    pub extra_minutes: i64,
    /// Path to the vocabulary text file used for this group.
    pub words_file: String,
    /// How often the rule resets: "daily", "weekly", or "monthly".
    pub reset_time: String,
}

pub fn open_config_db(path: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS whitelist (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            pattern    TEXT    NOT NULL UNIQUE,
            enabled    INTEGER NOT NULL DEFAULT 1,
            uid        INTEGER,
            group_name TEXT    NOT NULL DEFAULT ''
        );
        CREATE TABLE IF NOT EXISTS rules (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            group_name     TEXT    NOT NULL UNIQUE,
            reset_behavior TEXT    NOT NULL CHECK(reset_behavior IN ('daily','weekly','monthly')),
            \"limit\"      INTEGER NOT NULL
        );
        CREATE TABLE IF NOT EXISTS vocab_rules (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            group_name     TEXT    NOT NULL,
            correct_needed INTEGER NOT NULL DEFAULT 5,
            extra_minutes  INTEGER NOT NULL DEFAULT 15,
            words_file     TEXT    NOT NULL,
            reset_time     TEXT    NOT NULL DEFAULT 'daily'
                CHECK(reset_time IN ('daily','weekly','monthly'))
        );
        CREATE TABLE IF NOT EXISTS time_credits (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            vocab_rule_id INTEGER NOT NULL,
            earned_secs   INTEGER NOT NULL,
            earned_at     INTEGER NOT NULL
        );",
    )?;
    // Migrations for tables that predate these columns
    let _ = conn.execute_batch("ALTER TABLE whitelist ADD COLUMN uid INTEGER;");
    let _ =
        conn.execute_batch("ALTER TABLE whitelist ADD COLUMN group_name TEXT NOT NULL DEFAULT '';");
    let _ = conn.execute_batch(
        "ALTER TABLE vocab_rules ADD COLUMN reset_time TEXT NOT NULL DEFAULT 'daily';",
    );
    // Remove the UNIQUE constraint on vocab_rules.group_name so multiple rules per group
    // are allowed.  SQLite cannot drop constraints in-place; rebuild the table instead.
    let vocab_ddl: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='vocab_rules'",
            [],
            |row| row.get(0),
        )
        .unwrap_or_default();
    if vocab_ddl.to_uppercase().contains("UNIQUE") {
        conn.execute_batch(
            "CREATE TABLE vocab_rules_new (
                id             INTEGER PRIMARY KEY AUTOINCREMENT,
                group_name     TEXT    NOT NULL,
                correct_needed INTEGER NOT NULL DEFAULT 5,
                extra_minutes  INTEGER NOT NULL DEFAULT 15,
                words_file     TEXT    NOT NULL,
                reset_time     TEXT    NOT NULL DEFAULT 'daily'
                    CHECK(reset_time IN ('daily','weekly','monthly'))
            );
            INSERT INTO vocab_rules_new
                SELECT id, group_name, correct_needed, extra_minutes, words_file, reset_time
                FROM vocab_rules;
            DROP TABLE vocab_rules;
            ALTER TABLE vocab_rules_new RENAME TO vocab_rules;",
        )?;
    }
    // Rebuild time_credits if it still has the old group_name column.
    let has_old_schema: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('time_credits') WHERE name='group_name'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
    if has_old_schema {
        conn.execute_batch(
            "DROP TABLE time_credits;
             CREATE TABLE time_credits (
                 id            INTEGER PRIMARY KEY AUTOINCREMENT,
                 vocab_rule_id INTEGER NOT NULL,
                 earned_secs   INTEGER NOT NULL,
                 earned_at     INTEGER NOT NULL
             );",
        )?;
    }
    Ok(conn)
}

pub fn list_rules(conn: &Connection) -> Result<Vec<Rule>> {
    let mut stmt =
        conn.prepare("SELECT id, group_name, reset_behavior, \"limit\" FROM rules ORDER BY id")?;
    let rules = stmt
        .query_map([], |row| {
            Ok(Rule {
                id: row.get(0)?,
                group_name: row.get(1)?,
                reset_behavior: row.get(2)?,
                limit: row.get(3)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rules)
}

pub fn set_rules(conn: &Connection, rules: &[Rule]) -> Result<()> {
    conn.execute("DELETE FROM rules", [])?;
    for r in rules {
        conn.execute(
            "INSERT INTO rules (group_name, reset_behavior, \"limit\") VALUES (?1, ?2, ?3)",
            rusqlite::params![r.group_name, r.reset_behavior, r.limit],
        )?;
    }
    Ok(())
}

pub fn load_whitelist(conn: &Connection) -> Whitelist {
    let rows: Vec<(Option<u32>, String)> =
        match conn.prepare("SELECT uid, pattern FROM whitelist WHERE enabled = 1") {
            Err(e) => {
                eprintln!("appmon: failed to query whitelist: {e}");
                return Whitelist { entries: vec![] };
            }
            Ok(mut stmt) => stmt
                .query_map([], |row| {
                    Ok((row.get::<_, Option<u32>>(0)?, row.get::<_, String>(1)?))
                })
                .map(|rows| rows.filter_map(|r| r.ok()).collect())
                .unwrap_or_default(),
        };

    let entries = rows
        .into_iter()
        .filter_map(|(uid, pat)| match Regex::new(&pat) {
            Ok(re) => Some((uid, re)),
            Err(e) => {
                eprintln!("appmon: invalid regex {pat:?}: {e}");
                None
            }
        })
        .collect();

    Whitelist { entries }
}

pub fn list_whitelist_entries(conn: &Connection) -> Result<Vec<WhitelistEntry>> {
    let mut stmt =
        conn.prepare("SELECT id, uid, group_name, pattern, enabled FROM whitelist ORDER BY id")?;
    let entries = stmt
        .query_map([], |row| {
            Ok(WhitelistEntry {
                id: row.get(0)?,
                uid: row.get(1)?,
                group_name: row.get(2)?,
                pattern: row.get(3)?,
                enabled: row.get::<_, i64>(4)? != 0,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(entries)
}

pub fn set_whitelist_patterns(conn: &Connection, patterns: &[WhitelistPattern]) -> Result<()> {
    conn.execute("DELETE FROM whitelist", [])?;
    for p in patterns {
        conn.execute(
            "INSERT INTO whitelist (uid, group_name, pattern, enabled) VALUES (?1, ?2, ?3, 1)",
            rusqlite::params![p.uid, p.group_name, p.pattern],
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Vocab rules
// ---------------------------------------------------------------------------

pub fn list_vocab_rules(conn: &Connection) -> Result<Vec<VocabRule>> {
    let mut stmt = conn.prepare(
        "SELECT id, group_name, correct_needed, extra_minutes, words_file, reset_time
         FROM vocab_rules ORDER BY id",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(VocabRule {
                id: row.get(0)?,
                group_name: row.get(1)?,
                correct_needed: row.get(2)?,
                extra_minutes: row.get(3)?,
                words_file: row.get(4)?,
                reset_time: row.get(5)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();
    Ok(rows)
}

/// Replace all vocab rules (mirrors `set_rules`).
pub fn set_vocab_rules(conn: &Connection, rules: &[VocabRule]) -> Result<()> {
    conn.execute("DELETE FROM vocab_rules", [])?;
    for r in rules {
        conn.execute(
            "INSERT INTO vocab_rules (group_name, correct_needed, extra_minutes, words_file, reset_time)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![r.group_name, r.correct_needed, r.extra_minutes, r.words_file, r.reset_time],
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Time credits
// ---------------------------------------------------------------------------

/// Record extra seconds earned via the vocabulary quiz.
pub fn insert_time_credit(
    conn: &Connection,
    vocab_rule_id: i64,
    earned_secs: i64,
    earned_at: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO time_credits (vocab_rule_id, earned_secs, earned_at) VALUES (?1, ?2, ?3)",
        rusqlite::params![vocab_rule_id, earned_secs, earned_at],
    )?;
    Ok(())
}

/// Sum all credits earned for `vocab_rule_id` since `since` (unix timestamp).
pub fn sum_credits_since(conn: &Connection, vocab_rule_id: i64, since: i64) -> Result<i64> {
    let total: i64 = conn.query_row(
        "SELECT COALESCE(SUM(earned_secs), 0) FROM time_credits
         WHERE vocab_rule_id = ?1 AND earned_at >= ?2",
        rusqlite::params![vocab_rule_id, since],
        |row| row.get(0),
    )?;
    Ok(total)
}

/// Returns the Unix timestamp of the start of the current reset period.
pub fn period_start(now: i64, reset_time: &str) -> i64 {
    use chrono::{Datelike, TimeZone, Utc};
    let dt = Utc
        .timestamp_opt(now, 0)
        .single()
        .expect("invalid timestamp");
    let start = match reset_time {
        "weekly" => {
            let monday = dt.date_naive()
                - chrono::Duration::days(dt.weekday().num_days_from_monday() as i64);
            monday.and_hms_opt(0, 0, 0).unwrap()
        }
        "monthly" => dt
            .date_naive()
            .with_day(1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap(),
        _ => dt.date_naive().and_hms_opt(0, 0, 0).unwrap(),
    };
    start.and_utc().timestamp()
}

/// Returns true if the vocab rule has already been used (a credit recorded) in the current period.
pub fn has_credit_in_period(
    conn: &Connection,
    vocab_rule_id: i64,
    reset_time: &str,
    now: i64,
) -> Result<bool> {
    let since = period_start(now, reset_time);
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM time_credits WHERE vocab_rule_id = ?1 AND earned_at >= ?2",
        rusqlite::params![vocab_rule_id, since],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}
