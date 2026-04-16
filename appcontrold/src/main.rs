mod config;
mod db;
mod notify;
mod proc;

extern crate vocab_trainer;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use notify::notify;
use rusqlite::Connection;

use clap::{Args, Parser, Subcommand, ValueEnum};
use db::ActiveProcess;
use log::{debug, error};

type ProcessKey = (u32, i64); // (pid, start_epoch)

#[derive(Clone, ValueEnum)]
enum LogLevel {
    Warn,
    Info,
    Debug,
}

#[derive(Parser)]
#[command(name = "appmon", about = "Application activity monitor")]
struct Cli {
    /// Directory where appmon.db and appmon_config.db are stored.
    /// Overridden by the DATA_DIR environment variable.
    #[arg(long, env = "DATA_DIR", default_value = ".")]
    data_dir: std::path::PathBuf,
    /// Set log level (warn, info, debug)
    #[arg(long, value_enum, default_value = "warn")]
    log_level: LogLevel,
    /// Enable debug log level (shorthand for --log-level debug)
    #[arg(short = 'd')]
    debug: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the monitoring daemon (default)
    Serve,
    /// Manage monitor configuration
    Config(ConfigArgs),
    /// Inspect tracked processes
    Proc(ProcArgs),
    /// Manage usage rules
    Rules(RulesArgs),
    /// Manage vocabulary quiz progress
    Vocab(VocabArgs),
}

#[derive(Args)]
struct ConfigArgs {
    #[command(subcommand)]
    subcommand: ConfigCommand,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Show the current whitelist
    Show,
    /// Edit the whitelist in $EDITOR
    Edit,
}

#[derive(Args)]
struct ProcArgs {
    #[command(subcommand)]
    subcommand: ProcCommand,
}

#[derive(Subcommand)]
enum ProcCommand {
    /// List processes
    List(ListArgs),
    /// Show all database entries for a named application
    Show(ProcShowArgs),
}

#[derive(Args)]
struct ProcShowArgs {
    /// Application name to look up
    name: String,
    /// Only show entries started within this duration ago.
    /// Accepts a number followed by: s (seconds), d (days), w (weeks), m (months), y (years).
    /// Example: 7d, 2w, 3m, 1y
    #[arg(long)]
    duration: Option<String>,
}

#[derive(Args)]
struct ListArgs {
    #[command(subcommand)]
    subcommand: ListCommand,
}

#[derive(Subcommand)]
enum ListCommand {
    /// List all currently tracked processes
    Current,
    /// Show all processes tracked today with cumulative run time
    Today,
}

#[derive(Args)]
struct RulesArgs {
    #[command(subcommand)]
    subcommand: RulesCommand,
}

#[derive(Subcommand)]
enum RulesCommand {
    /// List all rules
    Show,
    /// Edit rules in $EDITOR
    Edit,
    /// Manage vocabulary quiz rules
    Vocab(VocabRulesArgs),
}

#[derive(Args)]
struct VocabRulesArgs {
    #[command(subcommand)]
    subcommand: VocabRulesCommand,
}

#[derive(Subcommand)]
enum VocabRulesCommand {
    /// Show all vocabulary quiz rules
    Show,
    /// Edit vocabulary quiz rules in $EDITOR
    Edit,
}

// ── appmon vocab <subcommand> ────────────────────────────────────────────────

#[derive(Args)]
struct VocabArgs {
    #[command(subcommand)]
    subcommand: VocabCommand,
}

#[derive(Subcommand)]
enum VocabCommand {
    /// Show correct-answer counts for a user
    List(VocabUserArg),
    /// Edit correct-answer counts for a user in $EDITOR
    Edit(VocabEditCmdArgs),
}

#[derive(Args)]
struct VocabUserArg {
    /// Username to display or edit progress for
    user: String,
}

#[derive(Args)]
struct VocabEditCmdArgs {
    /// Username whose progress to edit
    user: String,
    /// Vocabulary file — pre-populates words with zero count so they appear in the editor
    #[arg(long)]
    words_file: Option<String>,
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before epoch")
        .as_secs() as i64
}

fn window_start_for_reset(now: i64, reset_behavior: &str) -> i64 {
    use chrono::{Datelike, TimeZone, Utc};
    let dt = Utc.timestamp_opt(now, 0).single().expect("invalid timestamp");
    let start = match reset_behavior {
        "daily" => dt.date_naive().and_hms_opt(0, 0, 0).unwrap(),
        "weekly" => {
            let monday = dt.date_naive()
                - chrono::Duration::days(dt.weekday().num_days_from_monday() as i64);
            monday.and_hms_opt(0, 0, 0).unwrap()
        }
        "monthly" => dt.date_naive().with_day(1).unwrap().and_hms_opt(0, 0, 0).unwrap(),
        _ => dt.date_naive().and_hms_opt(0, 0, 0).unwrap(),
    };
    start.and_utc().timestamp()
}

struct ExceededGroup {
    group_name: String,
    total_secs: i64,
    limit_mins: i64,
    reset_behavior: String,
    /// Per-app usage within the window, sorted by descending duration.
    app_usage: Vec<(String, i64)>,
}

/// Returns one entry per rule group that is currently exceeded.
/// Time credits earned via the vocabulary quiz are subtracted from usage.
fn check_limit_rules(
    data_conn: &Connection,
    config_conn: &Connection,
    rules: &[config::Rule],
    whitelist_entries: &[config::WhitelistEntry],
    vocab_rules: &[config::VocabRule],
    now: i64,
) -> Vec<ExceededGroup> {
    let mut exceeded: Vec<ExceededGroup> = Vec::new();

    for rule in rules {
        let window_start = window_start_for_reset(now, &rule.reset_behavior);

        let group_patterns: Vec<regex::Regex> = whitelist_entries
            .iter()
            .filter(|e| e.enabled && e.group_name == rule.group_name)
            .filter_map(|e| regex::Regex::new(&e.pattern).ok())
            .collect();
        if group_patterns.is_empty() {
            continue;
        }

        let records = match db::list_processes_active_today(data_conn, window_start, now) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("appmon: error querying window for rule {:?}: {e}", rule.group_name);
                continue;
            }
        };

        let mut per_app: HashMap<String, i64> = HashMap::new();
        for r in &records {
            if group_patterns.iter().any(|re| re.is_match(&r.name)) {
                let effective_start = r.start_time.max(window_start);
                let effective_end = r.end_time.unwrap_or(now).min(now);
                *per_app.entry(r.name.clone()).or_insert(0) +=
                    (effective_end - effective_start).max(0);
            }
        }

        let raw_secs: i64 = per_app.values().sum();

        // Subtract credits earned across all vocab rules for this group.
        let credits: i64 = vocab_rules
            .iter()
            .filter(|vr| vr.group_name == rule.group_name)
            .map(|vr| {
                let since = config::period_start(now, &vr.reset_time);
                config::sum_credits_since(config_conn, vr.id, since).unwrap_or(0)
            })
            .sum();
        let total_secs = (raw_secs - credits).max(0);

        if total_secs > rule.limit * 60 {
            let mut app_usage: Vec<(String, i64)> = per_app.into_iter().collect();
            app_usage.sort_by(|a, b| b.1.cmp(&a.1));
            exceeded.push(ExceededGroup {
                group_name: rule.group_name.clone(),
                total_secs,
                limit_mins: rule.limit,
                reset_behavior: rule.reset_behavior.clone(),
                app_usage,
            });
        }
    }

    exceeded
}

fn cmd_serve(data_dir: &std::path::Path) {
    let config_path = data_dir.join("appmon_config.db").to_string_lossy().into_owned();
    let data_path   = data_dir.join("appmon.db").to_string_lossy().into_owned();

    let config_conn = config::open_config_db(&config_path)
        .unwrap_or_else(|e| panic!("cannot open config DB {config_path:?}: {e}"));
    let data_conn = db::open_db(&data_path)
        .unwrap_or_else(|e| panic!("cannot open data DB {data_path:?}: {e}"));

    let boot_time = proc::read_boot_time()
        .unwrap_or_else(|e| panic!("cannot read boot time: {e}"));
    let clk_tck = proc::get_clk_tck();

    let boot_id = db::get_or_create_boot_session(&data_conn, boot_time)
        .unwrap_or_else(|e| panic!("cannot register boot session: {e}"));

    eprintln!("appmon: boot_id={boot_id}, boot_time={boot_time}, clk_tck={clk_tck}");

    let running = Arc::new(AtomicBool::new(true));
    let r = Arc::clone(&running);
    ctrlc::set_handler(move || {
        eprintln!("appmon: shutting down…");
        r.store(false, Ordering::SeqCst);
    })
    .expect("cannot install signal handler");

    let mut whitelist = config::load_whitelist(&config_conn);
    let mut active: HashMap<ProcessKey, ActiveProcess> = HashMap::new();

    // Initial scan
    for snap in proc::enumerate_processes(boot_time, clk_tck) {
        if !whitelist.matches(snap.uid, &snap.name) {
            continue;
        }
        log::debug!("handle process: {}", snap.name);
        let key = (snap.pid, snap.start_epoch);
        match db::insert_process(&data_conn, boot_id, &snap) {
            Ok(db_id) => {
                active.insert(
                    key,
                    ActiveProcess {
                        db_id,
                        start_epoch: snap.start_epoch,
                    },
                );
            }
            Err(e) => eprintln!("appmon: insert error for pid {}: {e}", snap.pid),
        }
    }

    eprintln!("appmon: tracking {} already-running processes", active.len());

    let mut next = now_secs();
    while running.load(Ordering::SeqCst) {
        let now = now_secs();
        if now < next {
            std::thread::sleep(std::time::Duration::from_secs(1));
            continue;
        }
        next = now + 15;
        debug!("Looop {} > {}", now, next);

        // Reload whitelist, entries, rules, and vocab rules every 30 seconds.
        whitelist = config::load_whitelist(&config_conn);
        let whitelist_entries = config::list_whitelist_entries(&config_conn)
            .unwrap_or_else(|e| { eprintln!("appmon: cannot reload whitelist entries: {e}"); vec![] });
        let rules = config::list_rules(&config_conn)
            .unwrap_or_else(|e| { eprintln!("appmon: cannot reload rules: {e}"); vec![] });
        let vocab_rules = config::list_vocab_rules(&config_conn)
            .unwrap_or_else(|e| { eprintln!("appmon: cannot reload vocab rules: {e}"); vec![] });

        // Check limit rules every cycle (credits already subtracted inside).
        let exceeded = check_limit_rules(&data_conn, &config_conn, &rules, &whitelist_entries, &vocab_rules, now);

        // Build current set
        let snapshots = proc::enumerate_processes(boot_time, clk_tck);
        let current: HashMap<ProcessKey, proc::ProcSnapshot> = snapshots
            .into_iter()
            .filter(|s| whitelist.matches(s.uid, &s.name))
            .map(|s| ((s.pid, s.start_epoch), s))
            .collect();

        // Notify affected users for each exceeded group.
        // One notification per uid — pick the first matching live process for the pid.
        for eg in &exceeded {
            let patterns: Vec<regex::Regex> = whitelist_entries
                .iter()
                .filter(|e| e.enabled && e.group_name == eg.group_name)
                .filter_map(|e| regex::Regex::new(&e.pattern).ok())
                .collect();

            // Pick the first vocab rule for this group that hasn't been used in its
            // current reset period.  Falls back to None (→ regular notify) only when
            // every rule has already been completed this period.
            let vocab_rule = vocab_rules
                .iter()
                .filter(|vr| vr.group_name == eg.group_name)
                .find(|vr| {
                    !config::has_credit_in_period(&config_conn, vr.id, &vr.reset_time, now)
                        .unwrap_or(false)
                });

            let mut notified_uids = std::collections::HashSet::new();
            for ((pid, _), snap) in &current {
                if patterns.iter().any(|re| re.is_match(&snap.name))
                    && notified_uids.insert(snap.uid)
                {
                    let app_used = eg.app_usage
                        .iter()
                        .find(|(name, _)| name == &snap.name)
                        .map(|(_, secs)| *secs)
                        .unwrap_or(0);

                    if let Some(vr) = vocab_rule {
                        // Offer the vocabulary quiz to earn extra time.
                        notify::vocab_quiz(
                            snap.uid,
                            *pid,
                            eg.group_name.clone(),
                            vr.clone(),
                            config_path.clone(),
                            data_dir.to_string_lossy().into_owned(),
                        );
                    } else {
                        let msg = format!(
                            "# Usage limit reached\n\n\n\
                             **{}** has used {} of its {} {} allowance.",
                            snap.name,
                            format_duration(app_used),
                            format_limit(eg.limit_mins),
                            eg.reset_behavior,
                        );
                        notify(snap.uid, *pid, &msg);
                    }
                }
            }
        }

        // Departed processes
        let departed: Vec<ProcessKey> = active
            .keys()
            .filter(|k| !current.contains_key(*k))
            .copied()
            .collect();
        for key in departed {
            let ap = active.remove(&key).unwrap();
            let duration = now - ap.start_epoch;
            if let Err(e) = db::finalize_process(&data_conn, ap.db_id, now, duration) {
                eprintln!("appmon: finalize error for db_id {}: {e}", ap.db_id);
            }
        }

        // Arrived processes
        for (key, snap) in &current {
            if active.contains_key(key) {
                continue;
            }
            match db::insert_process(&data_conn, boot_id, snap) {
                Ok(db_id) => {
                    active.insert(
                        *key,
                        ActiveProcess {
                            db_id,
                            start_epoch: snap.start_epoch,
                        },
                    );
                }
                Err(e) => eprintln!("appmon: insert error for pid {}: {e}", snap.pid),
            }
        }
    }

    // Finalize all remaining active processes on shutdown
    let now = now_secs();
    for (_key, ap) in active {
        let duration = now - ap.start_epoch;
        if let Err(e) = db::finalize_process(&data_conn, ap.db_id, now, duration) {
            eprintln!("appmon: shutdown finalize error for db_id {}: {e}", ap.db_id);
        }
    }

    eprintln!("appmon: done.");
}

fn cmd_config_show(data_dir: &std::path::Path) {
    let path = data_dir.join("appmon_config.db").to_string_lossy().into_owned();
    let conn = config::open_config_db(&path)
        .unwrap_or_else(|e| panic!("cannot open config DB {path:?}: {e}"));
    let entries = config::list_whitelist_entries(&conn)
        .unwrap_or_else(|e| panic!("cannot read whitelist: {e}"));

    if entries.is_empty() {
        println!("whitelist is empty");
        return;
    }

    println!("{:<4}  {:<7}  {:<16}  {:<16}  pattern", "id", "enabled", "user", "group");
    println!("{}", "-".repeat(72));
    for e in &entries {
        let user = e.uid.map_or("*".to_string(), |u| {
            proc::uid_to_username(u).unwrap_or_else(|| u.to_string())
        });
        println!(
            "{:<4}  {:<7}  {:<16}  {:<16}  {}",
            e.id,
            if e.enabled { "yes" } else { "no" },
            user,
            e.group_name,
            e.pattern
        );
    }
}

fn cmd_config_edit(data_dir: &std::path::Path) {
    let path = data_dir.join("appmon_config.db").to_string_lossy().into_owned();
    let conn = config::open_config_db(&path)
        .unwrap_or_else(|e| panic!("cannot open config DB {path:?}: {e}"));

    let entries = config::list_whitelist_entries(&conn)
        .unwrap_or_else(|e| panic!("cannot read whitelist: {e}"));

    let tmp_path = "/tmp/appmon_whitelist_edit.txt";

    let mut content = String::from(
        "# appmon whitelist — one entry per line: user group pattern\n\
         # user    : username, numeric UID, or * to match any user\n\
         # group   : application group name (no spaces)\n\
         # pattern : regex matched against the process name\n\
         # Lines starting with '#' are ignored\n\n",
    );
    for e in &entries {
        let user = e.uid.map_or("*".to_string(), |u| {
            proc::uid_to_username(u).unwrap_or_else(|| u.to_string())
        });
        if e.enabled {
            content.push_str(&format!("{} {} {}", user, e.group_name, e.pattern));
        } else {
            content.push_str(&format!("# (disabled) {} {} {}", user, e.group_name, e.pattern));
        }
        content.push('\n');
    }

    std::fs::write(tmp_path, &content)
        .unwrap_or_else(|e| panic!("cannot write temp file: {e}"));

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());

    let status = std::process::Command::new(&editor)
        .arg(tmp_path)
        .status()
        .unwrap_or_else(|e| panic!("cannot launch editor '{editor}': {e}"));

    if !status.success() {
        eprintln!("appmon: editor exited with non-zero status, aborting");
        std::process::exit(1);
    }

    let edited = std::fs::read_to_string(tmp_path)
        .unwrap_or_else(|e| panic!("cannot read temp file: {e}"));

    let mut valid_patterns: Vec<config::WhitelistPattern> = Vec::new();
    for line in edited.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Format: user group pattern  (3 space-separated fields)
        let mut fields = line.splitn(3, char::is_whitespace);
        let user_part = match fields.next() {
            Some(f) => f,
            None => continue,
        };
        let group_part = match fields.next() {
            Some(f) => f,
            None => {
                eprintln!("appmon: skipping malformed entry (expected: user group pattern): {line:?}");
                continue;
            }
        };
        let pat = match fields.next().map(str::trim) {
            Some(f) if !f.is_empty() => f,
            _ => {
                eprintln!("appmon: skipping malformed entry (expected: user group pattern): {line:?}");
                continue;
            }
        };
        let uid: Option<u32> = if user_part == "*" {
            None
        } else if let Ok(u) = user_part.parse::<u32>() {
            Some(u)
        } else if let Some(u) = proc::username_to_uid(user_part) {
            Some(u)
        } else {
            eprintln!("appmon: skipping entry with unknown user {user_part:?}: {line:?}");
            continue;
        };
        match regex::Regex::new(pat) {
            Ok(_) => valid_patterns.push(config::WhitelistPattern {
                uid,
                group_name: group_part.to_string(),
                pattern: pat.to_string(),
            }),
            Err(e) => eprintln!("appmon: skipping invalid regex {pat:?}: {e}"),
        }
    }

    config::set_whitelist_patterns(&conn, &valid_patterns)
        .unwrap_or_else(|e| panic!("cannot update whitelist: {e}"));

    println!("appmon: whitelist updated ({} pattern(s))", valid_patterns.len());

    let _ = std::fs::remove_file(tmp_path);
}

fn format_duration(secs: i64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

fn cmd_proc_list_today(data_dir: &std::path::Path) {
    let path = data_dir.join("appmon.db").to_string_lossy().into_owned();
    let conn = db::open_db(&path)
        .unwrap_or_else(|e| panic!("cannot open data DB {path:?}: {e}"));

    let now = now_secs();
    let today_start = now - (now % 86400);

    let records = db::list_processes_active_today(&conn, today_start, now)
        .unwrap_or_else(|e| panic!("cannot read processes: {e}"));

    if records.is_empty() {
        println!("no processes tracked today");
        return;
    }

    // Accumulate effective duration per process name within today's window
    let mut totals: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for r in &records {
        let effective_start = r.start_time.max(today_start);
        let effective_end = r.end_time.unwrap_or(now).min(now);
        let duration = (effective_end - effective_start).max(0);
        *totals.entry(r.name.clone()).or_insert(0) += duration;
    }

    let mut rows: Vec<(String, i64)> = totals.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1));

    println!("{:<20}  total time", "name");
    println!("{}", "-".repeat(34));
    for (name, secs) in &rows {
        println!("{:<20}  {}", name, format_duration(*secs));
    }
}

fn cmd_proc_show(data_dir: &std::path::Path, name: &str, duration: Option<&str>) {
    use chrono::{DateTime, Utc};

    let since: Option<i64> = match duration {
        Some(d) => {
            let secs = parse_duration_secs(d)
                .unwrap_or_else(|e| { eprintln!("appmon: {e}"); std::process::exit(1); });
            Some(now_secs() - secs)
        }
        None => None,
    };

    let path = data_dir.join("appmon.db").to_string_lossy().into_owned();
    let conn = db::open_db(&path)
        .unwrap_or_else(|e| panic!("cannot open data DB {path:?}: {e}"));

    let records = db::list_entries_by_name(&conn, name, since)
        .unwrap_or_else(|e| panic!("cannot read entries: {e}"));

    if records.is_empty() {
        println!("no entries found for {name:?}");
        return;
    }

    println!(
        "{:<6}  {:<6}  {:<16}  {:<20}  {:<20}  {:<9}  cmdline",
        "db_id", "pid", "user", "start", "end", "duration",
    );
    println!("{}", "-".repeat(108));
    for r in &records {
        let fmt = |ts: i64| {
            DateTime::<Utc>::from_timestamp(ts, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| ts.to_string())
        };
        let user = proc::uid_to_username(r.uid)
            .unwrap_or_else(|| r.uid.to_string());
        let start = fmt(r.start_time);
        let end = r.end_time.map(fmt).unwrap_or_else(|| "running".to_string());
        let duration = r.duration_seconds
            .map(format_duration)
            .unwrap_or_else(|| "-".to_string());
        let cmdline = if r.cmdline.len() > 30 {
            format!("{}…", &r.cmdline[..29])
        } else {
            r.cmdline.clone()
        };
        println!(
            "{:<6}  {:<6}  {:<16}  {:<20}  {:<20}  {:<9}  {}",
            r.id, r.pid, user, start, end, duration, cmdline,
        );
    }
}

fn cmd_proc_list(data_dir: &std::path::Path) {
    let data_path = data_dir.join("appmon.db").to_string_lossy().into_owned();
    let config_path = data_dir.join("appmon_config.db").to_string_lossy().into_owned();

    let data_conn = db::open_db(&data_path)
        .unwrap_or_else(|e| panic!("cannot open data DB {data_path:?}: {e}"));
    let config_conn = config::open_config_db(&config_path)
        .unwrap_or_else(|e| panic!("cannot open config DB {config_path:?}: {e}"));

    let records = db::list_active_processes(&data_conn)
        .unwrap_or_else(|e| panic!("cannot read processes: {e}"));

    if records.is_empty() {
        println!("no tracked processes");
        return;
    }

    let now = now_secs();

    let whitelist_entries = config::list_whitelist_entries(&config_conn)
        .unwrap_or_else(|e| { eprintln!("appmon: cannot load whitelist entries: {e}"); vec![] });
    let rules = config::list_rules(&config_conn)
        .unwrap_or_else(|e| { eprintln!("appmon: cannot load rules: {e}"); vec![] });
    let vocab_rules = config::list_vocab_rules(&config_conn)
        .unwrap_or_else(|e| { eprintln!("appmon: cannot load vocab rules: {e}"); vec![] });

    // Compute used seconds and vocab credits per rule group within its reset window.
    // group_name -> (raw_used_secs, limit_mins, extra_secs)
    let mut group_usage: HashMap<String, (i64, i64, i64)> = HashMap::new();
    for rule in &rules {
        let window_start = window_start_for_reset(now, &rule.reset_behavior);
        let patterns: Vec<regex::Regex> = whitelist_entries
            .iter()
            .filter(|e| e.enabled && e.group_name == rule.group_name)
            .filter_map(|e| regex::Regex::new(&e.pattern).ok())
            .collect();
        if patterns.is_empty() {
            continue;
        }
        let today_records = db::list_processes_active_today(&data_conn, window_start, now)
            .unwrap_or_default();
        let mut total_secs: i64 = 0;
        for tr in &today_records {
            if patterns.iter().any(|re| re.is_match(&tr.name)) {
                let effective_start = tr.start_time.max(window_start);
                let effective_end = tr.end_time.unwrap_or(now).min(now);
                total_secs += (effective_end - effective_start).max(0);
            }
        }
        let extra_secs: i64 = vocab_rules
            .iter()
            .filter(|vr| vr.group_name == rule.group_name)
            .map(|vr| {
                let since = config::period_start(now, &vr.reset_time);
                config::sum_credits_since(&config_conn, vr.id, since).unwrap_or(0)
            })
            .sum();
        group_usage.insert(rule.group_name.clone(), (total_secs, rule.limit, extra_secs));
    }

    // Compiled patterns for matching a process name to its group.
    let name_to_group: Vec<(regex::Regex, String)> = whitelist_entries
        .iter()
        .filter(|e| e.enabled && !e.group_name.is_empty())
        .filter_map(|e| regex::Regex::new(&e.pattern).ok().map(|re| (re, e.group_name.clone())))
        .collect();

    println!(
        "{:<6}  {:<6}  {:<12}  {:<9}  {:<14}  {:<9}  {:<9}  {:<9}  cmdline",
        "db_id", "pid", "name", "running", "group", "used", "extra", "left",
    );
    println!("{}", "-".repeat(108));
    for r in &records {
        let running = format_duration(now - r.start_time);

        let group = name_to_group
            .iter()
            .find(|(re, _)| re.is_match(&r.name))
            .map(|(_, g)| g.as_str())
            .unwrap_or("-");

        let (used_str, extra_str, left_str) = match group_usage.get(group) {
            Some((raw_secs, limit_mins, extra_secs)) => {
                let limit_secs = limit_mins * 60;
                let effective_used = (raw_secs - extra_secs).max(0);
                let left_secs = limit_secs - effective_used;
                let left = if left_secs <= 0 {
                    "OVER".to_string()
                } else {
                    format_duration(left_secs)
                };
                let extra = if *extra_secs > 0 {
                    format!("+{}", format_duration(*extra_secs))
                } else {
                    "-".to_string()
                };
                (format_duration(effective_used), extra, left)
            }
            None => ("-".to_string(), "-".to_string(), "-".to_string()),
        };

        let cmdline = if r.cmdline.len() > 30 {
            format!("{}…", &r.cmdline[..29])
        } else {
            r.cmdline.clone()
        };
        println!(
            "{:<6}  {:<6}  {:<12}  {:<9}  {:<14}  {:<9}  {:<9}  {:<9}  {}",
            r.id, r.pid, r.name, running, group, used_str, extra_str, left_str, cmdline,
        );
    }
}

/// Parse a duration string such as `7d`, `2w`, `30s`, `3m`, `1y` into seconds.
fn parse_duration_secs(s: &str) -> Result<i64, String> {
    let s = s.trim();
    let (num_part, suffix) = s
        .find(|c: char| !c.is_ascii_digit())
        .map(|i| s.split_at(i))
        .unwrap_or((s, "s"));
    let n: i64 = num_part
        .parse()
        .map_err(|_| format!("invalid number in duration {s:?}"))?;
    let secs = match suffix {
        "s" | "" => n,
        "d"      => n * 86_400,
        "w"      => n * 7 * 86_400,
        "m"      => n * 30 * 86_400,
        "y"      => n * 365 * 86_400,
        other    => return Err(format!("unknown duration unit {other:?} (use s/d/w/m/y)")),
    };
    Ok(secs)
}

fn parse_limit(s: &str) -> Result<i64, String> {
    let mut total: i64 = 0;
    for term in s.split('+') {
        let term = term.trim();
        if term.is_empty() {
            continue;
        }
        if let Some(h) = term.strip_suffix('h') {
            let hours: i64 = h.trim().parse().map_err(|_| format!("invalid hours value: {h:?}"))?;
            total += hours * 60;
        } else if let Some(m) = term.strip_suffix('m') {
            let mins: i64 = m.trim().parse().map_err(|_| format!("invalid minutes value: {m:?}"))?;
            total += mins;
        } else {
            let mins: i64 = term.parse().map_err(|_| format!("invalid limit value: {term:?}"))?;
            total += mins;
        }
    }
    if total <= 0 {
        return Err(format!("limit must be greater than zero, got {total}"));
    }
    Ok(total)
}

fn format_limit(minutes: i64) -> String {
    let h = minutes / 60;
    let m = minutes % 60;
    match (h, m) {
        (0, _) => format!("{m}m"),
        (_, 0) => format!("{h}h"),
        _ => format!("{h}h+{m}m"),
    }
}

fn cmd_rules_show(data_dir: &std::path::Path) {
    let path = data_dir.join("appmon_config.db").to_string_lossy().into_owned();
    let conn = config::open_config_db(&path)
        .unwrap_or_else(|e| panic!("cannot open config DB {path:?}: {e}"));
    let rules = config::list_rules(&conn)
        .unwrap_or_else(|e| panic!("cannot read rules: {e}"));

    if rules.is_empty() {
        println!("no rules defined");
        return;
    }

    println!("{:<4}  {:<20}  {:<10}  limit", "id", "group", "reset");
    println!("{}", "-".repeat(50));
    for r in &rules {
        println!(
            "{:<4}  {:<20}  {:<10}  {}",
            r.id,
            r.group_name,
            r.reset_behavior,
            format_limit(r.limit),
        );
    }
}

fn cmd_rules_edit(data_dir: &std::path::Path) {
    let path = data_dir.join("appmon_config.db").to_string_lossy().into_owned();
    let conn = config::open_config_db(&path)
        .unwrap_or_else(|e| panic!("cannot open config DB {path:?}: {e}"));
    let rules = config::list_rules(&conn)
        .unwrap_or_else(|e| panic!("cannot read rules: {e}"));

    let tmp_path = "/tmp/appmon_rules_edit.txt";

    let mut content = String::from(
        "# appmon rules — one entry per line: group reset limit\n\
         # group : application group name (matches whitelist group column)\n\
         # reset : daily | weekly | monthly\n\
         # limit : minutes, e.g. 90  1h  30m  1h+30m\n\
         # Lines starting with '#' are ignored\n\n",
    );
    for r in &rules {
        content.push_str(&format!(
            "{} {} {}\n",
            r.group_name,
            r.reset_behavior,
            format_limit(r.limit),
        ));
    }

    std::fs::write(tmp_path, &content)
        .unwrap_or_else(|e| panic!("cannot write temp file: {e}"));

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());

    let status = std::process::Command::new(&editor)
        .arg(tmp_path)
        .status()
        .unwrap_or_else(|e| panic!("cannot launch editor '{editor}': {e}"));

    if !status.success() {
        eprintln!("appmon: editor exited with non-zero status, aborting");
        std::process::exit(1);
    }

    let edited = std::fs::read_to_string(tmp_path)
        .unwrap_or_else(|e| panic!("cannot read temp file: {e}"));

    let mut valid_rules: Vec<config::Rule> = Vec::new();
    for line in edited.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Format: group reset limit  (3 space-separated fields)
        let mut fields = line.splitn(3, char::is_whitespace);
        let group = match fields.next() {
            Some(f) => f.trim(),
            None => continue,
        };
        let reset = match fields.next() {
            Some(f) => f.trim(),
            None => {
                eprintln!("appmon: skipping malformed rule (expected: group reset limit): {line:?}");
                continue;
            }
        };
        let limit_str = match fields.next().map(str::trim) {
            Some(f) if !f.is_empty() => f,
            _ => {
                eprintln!("appmon: skipping malformed rule (expected: group reset limit): {line:?}");
                continue;
            }
        };
        if !matches!(reset, "daily" | "weekly" | "monthly") {
            eprintln!("appmon: skipping rule with invalid reset {reset:?} (expected daily/weekly/monthly): {line:?}");
            continue;
        }
        let limit = match parse_limit(limit_str) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("appmon: skipping rule with invalid limit {limit_str:?}: {e}");
                continue;
            }
        };
        valid_rules.push(config::Rule {
            id: 0, // assigned by DB
            group_name: group.to_string(),
            reset_behavior: reset.to_string(),
            limit,
        });
    }

    config::set_rules(&conn, &valid_rules)
        .unwrap_or_else(|e| panic!("cannot update rules: {e}"));

    println!("appmon: rules updated ({} rule(s))", valid_rules.len());

    let _ = std::fs::remove_file(tmp_path);
}

// ---------------------------------------------------------------------------
// rules vocab show / edit
// ---------------------------------------------------------------------------

fn cmd_vocab_rules_show(data_dir: &std::path::Path) {
    let path = data_dir.join("appmon_config.db").to_string_lossy().into_owned();
    let conn = config::open_config_db(&path)
        .unwrap_or_else(|e| panic!("cannot open config DB {path:?}: {e}"));
    let rules = config::list_vocab_rules(&conn)
        .unwrap_or_else(|e| panic!("cannot read vocab rules: {e}"));

    if rules.is_empty() {
        println!("no vocab rules defined");
        return;
    }

    println!(
        "{:<4}  {:<20}  {:<8}  {:<14}  {:<10}  words-file",
        "id", "group", "correct", "extra-minutes", "reset-time"
    );
    println!("{}", "-".repeat(84));
    for r in &rules {
        println!(
            "{:<4}  {:<20}  {:<8}  {:<14}  {:<10}  {}",
            r.id, r.group_name, r.correct_needed, r.extra_minutes, r.reset_time, r.words_file
        );
    }
}

fn cmd_vocab_rules_edit(data_dir: &std::path::Path) {
    let path = data_dir.join("appmon_config.db").to_string_lossy().into_owned();
    let conn = config::open_config_db(&path)
        .unwrap_or_else(|e| panic!("cannot open config DB {path:?}: {e}"));
    let rules = config::list_vocab_rules(&conn)
        .unwrap_or_else(|e| panic!("cannot read vocab rules: {e}"));

    let tmp_path = "/tmp/appmon_vocab_rules_edit.txt";

    let mut content = String::from(
        "# appmon vocab rules — one entry per line: group correct extra-minutes reset-time words-file\n\
         # group         : application group name (matches whitelist group column)\n\
         # correct       : correct answers needed to earn extra time\n\
         # extra-minutes : minutes of extra time awarded\n\
         # reset-time    : how often the rule resets: daily, weekly, monthly\n\
         # words-file    : path to the vocabulary text file\n\
         # Lines starting with '#' are ignored\n\n",
    );
    for r in &rules {
        content.push_str(&format!(
            "{} {} {} {} {}\n",
            r.group_name, r.correct_needed, r.extra_minutes, r.reset_time, r.words_file
        ));
    }

    std::fs::write(tmp_path, &content)
        .unwrap_or_else(|e| panic!("cannot write temp file: {e}"));

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());

    let status = std::process::Command::new(&editor)
        .arg(tmp_path)
        .status()
        .unwrap_or_else(|e| panic!("cannot launch editor '{editor}': {e}"));

    if !status.success() {
        eprintln!("appmon: editor exited with non-zero status, aborting");
        std::process::exit(1);
    }

    let edited = std::fs::read_to_string(tmp_path)
        .unwrap_or_else(|e| panic!("cannot read temp file: {e}"));

    let mut valid_rules: Vec<config::VocabRule> = Vec::new();
    for line in edited.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Format: group correct extra-minutes reset-time words-file (words-file may contain spaces)
        let mut fields = line.splitn(5, char::is_whitespace);
        let group = match fields.next().map(str::trim) {
            Some(f) if !f.is_empty() => f,
            _ => continue,
        };
        let correct_str = match fields.next().map(str::trim) {
            Some(f) if !f.is_empty() => f,
            _ => {
                eprintln!("appmon: skipping malformed vocab rule (expected: group correct extra-minutes reset-time words-file): {line:?}");
                continue;
            }
        };
        let extra_str = match fields.next().map(str::trim) {
            Some(f) if !f.is_empty() => f,
            _ => {
                eprintln!("appmon: skipping malformed vocab rule: {line:?}");
                continue;
            }
        };
        let reset_time = match fields.next().map(str::trim) {
            Some(f) if matches!(f, "daily" | "weekly" | "monthly") => f,
            Some(other) => {
                eprintln!("appmon: skipping vocab rule with invalid reset-time {other:?} (expected daily, weekly, or monthly): {line:?}");
                continue;
            }
            _ => {
                eprintln!("appmon: skipping vocab rule with missing reset-time: {line:?}");
                continue;
            }
        };
        let words_file = match fields.next().map(str::trim) {
            Some(f) if !f.is_empty() => f,
            _ => {
                eprintln!("appmon: skipping vocab rule with missing words-file: {line:?}");
                continue;
            }
        };
        let correct_needed: i64 = match correct_str.parse() {
            Ok(v) if v > 0 => v,
            _ => {
                eprintln!("appmon: skipping vocab rule with invalid correct value {correct_str:?}");
                continue;
            }
        };
        let extra_minutes: i64 = match extra_str.parse() {
            Ok(v) if v > 0 => v,
            _ => {
                eprintln!("appmon: skipping vocab rule with invalid extra-minutes value {extra_str:?}");
                continue;
            }
        };
        valid_rules.push(config::VocabRule {
            id: 0,
            group_name: group.to_string(),
            correct_needed,
            extra_minutes,
            reset_time: reset_time.to_string(),
            words_file: words_file.to_string(),
        });
    }

    config::set_vocab_rules(&conn, &valid_rules)
        .unwrap_or_else(|e| panic!("cannot update vocab rules: {e}"));

    println!("appmon: vocab rules updated ({} rule(s))", valid_rules.len());

    let _ = std::fs::remove_file(tmp_path);
}

// ---------------------------------------------------------------------------
// vocab list / edit (progress per user)
// ---------------------------------------------------------------------------

fn progress_db_path(data_dir: &std::path::Path, uid: u32) -> String {
    format!("/tmp/vocab_progress_{uid}.db")
}

fn cmd_vocab_list(data_dir: &std::path::Path, username: &str) {
    let uid = match proc::username_to_uid(username) {
        Some(u) => u,
        None => {
            eprintln!("appmon: unknown user {username:?}");
            std::process::exit(1);
        }
    };
    let db_path = progress_db_path(data_dir, uid);
    vocab_trainer::print_progress(&db_path, username);
}

fn cmd_vocab_edit(data_dir: &std::path::Path, username: &str, words_file: Option<&str>) {
    let uid = match proc::username_to_uid(username) {
        Some(u) => u,
        None => {
            // Fall back to looking for words-file from vocab_rules.
            eprintln!("appmon: unknown user {username:?}");
            std::process::exit(1);
        }
    };
    let db_path = progress_db_path(data_dir, uid);

    // If no words-file given, try to find one from vocab_rules.
    let derived_words_file: Option<String>;
    let effective_words_file = if words_file.is_some() {
        words_file
    } else {
        let config_path = data_dir.join("appmon_config.db").to_string_lossy().into_owned();
        if let Ok(conn) = config::open_config_db(&config_path) {
            if let Ok(rules) = config::list_vocab_rules(&conn) {
                derived_words_file = rules.into_iter().next().map(|r| r.words_file);
                derived_words_file.as_deref()
            } else {
                derived_words_file = None;
                None
            }
        } else {
            derived_words_file = None;
            None
        }
    };

    vocab_trainer::edit_progress(&db_path, effective_words_file, username);
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    let cli = Cli::parse();
    let level = if cli.debug {
        log::LevelFilter::Debug
    } else {
        match cli.log_level {
            LogLevel::Warn  => log::LevelFilter::Warn,
            LogLevel::Info  => log::LevelFilter::Info,
            LogLevel::Debug => log::LevelFilter::Debug,
        }
    };
    env_logger::Builder::new().filter_level(level).init();
    let data_dir = &cli.data_dir;
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => cmd_serve(data_dir),
        Command::Config(args) => match args.subcommand {
            ConfigCommand::Show => cmd_config_show(data_dir),
            ConfigCommand::Edit => cmd_config_edit(data_dir),
        },
        Command::Proc(args) => match args.subcommand {
            ProcCommand::List(list_args) => match list_args.subcommand {
                ListCommand::Current => cmd_proc_list(data_dir),
                ListCommand::Today => cmd_proc_list_today(data_dir),
            },
            ProcCommand::Show(a) => cmd_proc_show(data_dir, &a.name, a.duration.as_deref()),
        },
        Command::Rules(args) => match args.subcommand {
            RulesCommand::Show => cmd_rules_show(data_dir),
            RulesCommand::Edit => cmd_rules_edit(data_dir),
            RulesCommand::Vocab(vargs) => match vargs.subcommand {
                VocabRulesCommand::Show => cmd_vocab_rules_show(data_dir),
                VocabRulesCommand::Edit => cmd_vocab_rules_edit(data_dir),
            },
        },
        Command::Vocab(args) => match args.subcommand {
            VocabCommand::List(a) => cmd_vocab_list(data_dir, &a.user),
            VocabCommand::Edit(a) => {
                cmd_vocab_edit(data_dir, &a.user, a.words_file.as_deref())
            }
        },
    }
}
