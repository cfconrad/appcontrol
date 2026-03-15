use regex::Regex;
use rusqlite::{Connection, Result};

pub struct Whitelist {
    pub entries: Vec<(Option<u32>, Regex)>,
}

impl Whitelist {
    pub fn matches(&self, uid: u32, name: &str) -> bool {
        self.entries.iter().any(|(entry_uid, re)| {
            entry_uid.map_or(true, |u| u == uid) && re.is_match(name)
        })
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
        );",
    )?;
    // Migrations for tables that predate these columns
    let _ = conn.execute_batch("ALTER TABLE whitelist ADD COLUMN uid INTEGER;");
    let _ = conn.execute_batch(
        "ALTER TABLE whitelist ADD COLUMN group_name TEXT NOT NULL DEFAULT '';",
    );
    Ok(conn)
}

pub fn list_rules(conn: &Connection) -> Result<Vec<Rule>> {
    let mut stmt = conn.prepare(
        "SELECT id, group_name, reset_behavior, \"limit\" FROM rules ORDER BY id",
    )?;
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
    let rows: Vec<(Option<u32>, String)> = match conn.prepare(
        "SELECT uid, pattern FROM whitelist WHERE enabled = 1",
    ) {
        Err(e) => {
            eprintln!("appmon: failed to query whitelist: {e}");
            return Whitelist { entries: vec![] };
        }
        Ok(mut stmt) => stmt
            .query_map([], |row| Ok((row.get::<_, Option<u32>>(0)?, row.get::<_, String>(1)?)))
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
    let mut stmt = conn.prepare(
        "SELECT id, uid, group_name, pattern, enabled FROM whitelist ORDER BY id",
    )?;
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
