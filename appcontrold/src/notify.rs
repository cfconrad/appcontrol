use std::collections::{HashMap, HashSet};
use std::sync::{LazyLock, Mutex};
use log;

use crate::config::VocabRule;
use crate::proc::{uid_to_home_dir, uid_to_username};

/// Number of times a uid can click "Ok" before only "Close Application" remains.
const WARN_LIMIT: u32 = 3;

/// Seconds to wait before re-showing the popup after "Ok" is clicked.
const REPEAT_SECS: u64 = 30;

/// Seconds the dialog may stay open before the application is force-killed.
const DIALOG_TIMEOUT_SECS: u64 = 15;

/// Per-uid notification counts (how many times the popup has been shown).
static COUNTS: LazyLock<Mutex<HashMap<u32, u32>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// PIDs that currently have an active notification loop.
static ACTIVE_PIDS: LazyLock<Mutex<HashSet<u32>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Show a notification popup for the given user with the supplied message.
///
/// The popup has two buttons:
/// - "Ok": dismisses the popup; it will reappear after [`REPEAT_SECS`] seconds.
/// - "Close Application": sends SIGTERM to all processes owned by `uid`.
///
/// After the popup has been shown [`WARN_LIMIT`] times (i.e. the user clicked "Ok"
/// that many times), only the "Close Application" button is presented.
///
/// Before each popup the process identified by `pid` is checked; if it is no
/// longer running the notification loop exits silently.
///
/// Returns immediately; the dialog loop runs in a background thread.
pub fn notify(uid: u32, pid: u32, message: &str) {
    let msg = message.to_string();
    std::thread::spawn(move || run_notify_loop(uid, pid, msg));
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum DialogResult {
    Ok,
    CloseApplication,
    Failed,
}

fn is_process_running(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

fn run_notify_loop(uid: u32, pid: u32, message: String) {
    // Ensure only one notification loop runs per PID.
    if !ACTIVE_PIDS.lock().unwrap().insert(pid) {
        log::warn!("notify: loop already active for pid {pid}, skipping");
        return;
    }

    loop {
        log::debug!("notify: loop for uid:{} pid:{}", uid, pid);
        if !is_process_running(pid) {
            break;
        }

        let count = {
            let mut map = COUNTS.lock().unwrap();
            let c = map.entry(uid).or_insert(0);
            *c += 1;
            *c
        };

        let warn_only = count >= WARN_LIMIT;

        match show_dialog(uid, pid, &message, warn_only) {
            DialogResult::CloseApplication => {
                kill_processe(pid);
                COUNTS.lock().unwrap().remove(&uid);
                break;
            }
            DialogResult::Ok => {
                // Sleep then loop — the count will increment on the next pass.
                std::thread::sleep(std::time::Duration::from_secs(REPEAT_SECS));
            }
            DialogResult::Failed => {
                eprintln!("appmon notify: dialog failed for uid {uid}, giving up");
                break;
            }
        }
    }

    ACTIVE_PIDS.lock().unwrap().remove(&pid);
}

/// Spawn a popup dialog as the target user on their display using the xpopup library.
///
/// If the dialog stays open longer than [`DIALOG_TIMEOUT_SECS`] without the
/// user clicking a button, the monitored application (`app_pid`) is killed
/// immediately via SIGTERM.
fn show_dialog(uid: u32, app_pid: u32, message: &str, warn_only: bool) -> DialogResult {
    let env = match find_user_display_env(uid) {
        Some(e) => e,
        None => {
            eprintln!("appmon notify: cannot find display session for uid {uid}");
            return DialogResult::Failed;
        }
    };

    let gid = get_user_gid(uid).unwrap_or(uid);

    unsafe {
        let pid = libc::fork();
        if pid < 0 {
            eprintln!("appmon notify: fork failed");
            return DialogResult::Failed;
        }

        if pid == 0 {
            // Child process: Drop privileges and run the popup

            // Clear current env to avoid leaking daemon env (like APPMON_DB)
            let keys: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
            for k in keys {
                std::env::remove_var(k);
            }

            // Set environment variables for the user session
            for (key, val) in &env {
                std::env::set_var(key, val);
            }

            // Drop privileges
            if libc::setresgid(gid, gid, gid) != 0 {
                libc::_exit(1);
            }
            if libc::setresuid(uid, uid, uid) != 0 {
                libc::_exit(1);
            }

            // Call the xpopup library function
            xpopup::run_popup(message.to_string(), None, warn_only, Some(DIALOG_TIMEOUT_SECS));

            // Should not be reached as run_popup calls process::exit
            libc::_exit(0);
        } else {
            // Parent process: poll for child exit, enforcing a dialog timeout.
            let deadline = std::time::Instant::now()
                + std::time::Duration::from_secs(DIALOG_TIMEOUT_SECS);

            loop {
                let mut status = 0;
                let ret = libc::waitpid(pid, &mut status, libc::WNOHANG);

                if ret > 0 {
                    return if libc::WIFEXITED(status) {
                        let exit_code = libc::WEXITSTATUS(status);
                        match exit_code {
                            0 => DialogResult::Ok,
                            2 => DialogResult::CloseApplication,
                            _ => {
                                eprintln!("appmon notify: xpopup exited with code {exit_code}");
                                DialogResult::Failed
                            }
                        }
                    } else {
                        eprintln!("appmon notify: xpopup terminated abnormally");
                        DialogResult::Failed
                    };
                }

                if std::time::Instant::now() >= deadline {
                    // Dialog timed out — kill the xpopup child and the app.
                    libc::kill(pid, libc::SIGTERM);
                    libc::waitpid(pid, &mut status, 0);
                    kill_processe(app_pid);
                    return DialogResult::CloseApplication;
                }

                std::thread::sleep(std::time::Duration::from_millis(250));
            }
        }
    }
}

/// Walk `/proc` to find a process owned by `uid` that has `DISPLAY` or
/// `WAYLAND_DISPLAY` set, and return its full environment.
fn find_user_display_env(uid: u32) -> Option<HashMap<String, String>> {
    let proc = std::fs::read_dir("/proc").ok()?;

    for entry in proc.flatten() {
        // Only numeric entries (PIDs).
        if !entry
            .file_name()
            .to_string_lossy()
            .bytes()
            .all(|b| b.is_ascii_digit())
        {
            continue;
        }

        let base = entry.path();

        // Check the real UID from /proc/<pid>/status.
        let status = match std::fs::read_to_string(base.join("status")) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let proc_uid: u32 = status
            .lines()
            .find(|l| l.starts_with("Uid:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(u32::MAX);

        if proc_uid != uid {
            continue;
        }

        // Parse the null-separated environ file.
        let data = match std::fs::read(base.join("environ")) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let mut env: HashMap<String, String> = HashMap::new();
        for item in data.split(|&b| b == 0) {
            if let Some(pos) = item.iter().position(|&b| b == b'=') {
                let key = String::from_utf8_lossy(&item[..pos]).into_owned();
                let val = String::from_utf8_lossy(&item[pos + 1..]).into_owned();
                env.insert(key, val);
            }
        }

        if env.contains_key("DISPLAY") || env.contains_key("WAYLAND_DISPLAY") {
            return Some(env);
        }
    }

    None
}

/// Look up the primary GID for a uid via `getpwuid_r`.
fn get_user_gid(uid: u32) -> Option<u32> {
    let buf_size = {
        let s = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
        if s > 0 { s as usize } else { 1024 }
    };

    let mut buf = vec![0 as libc::c_char; buf_size];
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result: *mut libc::passwd = std::ptr::null_mut();

    let ret = unsafe {
        libc::getpwuid_r(uid, pwd.as_mut_ptr(), buf.as_mut_ptr(), buf.len(), &mut result)
    };

    if ret != 0 || result.is_null() {
        return None;
    }

    Some(unsafe { pwd.assume_init().pw_gid })
}

fn kill_processe(pid: u32) {
    if !is_process_running(pid) {
        return;
    }
    unsafe {
        libc::kill(pid.try_into().unwrap(), libc::SIGTERM);
    }
}

// ---------------------------------------------------------------------------
// Vocabulary quiz notification
// ---------------------------------------------------------------------------

/// Group names that currently have a quiz loop running (prevents duplicate loops).
static ACTIVE_QUIZ_GROUPS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Offer a vocabulary quiz to the user before showing the usual "limit reached" popup.
///
/// If the user completes the quiz (exits 0), extra time is recorded in the config DB.
/// If the user quits the quiz (exits 1), the regular `notify()` popup is shown instead.
///
/// Returns immediately; the quiz loop runs in a background thread.
pub fn vocab_quiz(
    uid: u32,
    pid: u32,
    group_name: String,
    rule: VocabRule,
    config_db_path: String,
    data_dir: String,
) {
    std::thread::spawn(move || {
        run_quiz_loop(uid, pid, group_name, rule, config_db_path, data_dir)
    });
}

fn run_quiz_loop(
    uid: u32,
    pid: u32,
    group_name: String,
    rule: VocabRule,
    config_db_path: String,
    data_dir: String,
) {
    // Only one quiz loop per group at a time.
    if !ACTIVE_QUIZ_GROUPS.lock().unwrap().insert(group_name.clone()) {
        log::debug!("vocab_quiz: loop already active for group {group_name:?}, skipping");
        return;
    }

    if !is_process_running(pid) {
        ACTIVE_QUIZ_GROUPS.lock().unwrap().remove(&group_name);
        return;
    }

    // Skip the quiz if it was already completed in the current reset period.
    let already_used = match crate::config::open_config_db(&config_db_path) {
        Ok(conn) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            crate::config::has_credit_in_period(&conn, rule.id, &rule.reset_time, now)
                .unwrap_or(false)
        }
        Err(e) => {
            eprintln!("appmon vocab_quiz: cannot open config DB to check period: {e}");
            false
        }
    };
    if already_used {
        notify(uid, pid, "# Usage limit reached\n\nYou have reached your usage limit.\nThe vocabulary quiz has already been completed for this period.");
        ACTIVE_QUIZ_GROUPS.lock().unwrap().remove(&group_name);
        return;
    }

    let result = show_quiz(uid, &rule, &data_dir);

    match result {
        QuizOutcome::Earned(earned_secs) => {
            // Record the credit in the config DB.
            match crate::config::open_config_db(&config_db_path) {
                Ok(conn) => {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;
                    if let Err(e) = crate::config::insert_time_credit(
                        &conn,
                        rule.id,
                        earned_secs,
                        now,
                    ) {
                        eprintln!("appmon vocab_quiz: failed to record credit: {e}");
                    }
                }
                Err(e) => eprintln!("appmon vocab_quiz: cannot open config DB: {e}"),
            }
        }
        QuizOutcome::Quit => {
            // Fall back to the regular notification popup.
            notify(uid, pid, "# Usage limit reached\n\nYou have reached your usage limit.\nClose the application or earn more time with the vocabulary quiz.");
        }
        QuizOutcome::Failed => {
            eprintln!("appmon vocab_quiz: quiz process failed for uid {uid}");
        }
    }

    ACTIVE_QUIZ_GROUPS.lock().unwrap().remove(&group_name);
}

#[derive(Debug)]
enum QuizOutcome {
    Earned(i64), // seconds earned
    Quit,
    Failed,
}

/// Fork a child process, drop privileges, and run the vocabulary quiz.
fn show_quiz(uid: u32, rule: &VocabRule, data_dir: &str) -> QuizOutcome {
    let env = match find_user_display_env(uid) {
        Some(e) => e,
        None => {
            eprintln!("appmon vocab_quiz: cannot find display session for uid {uid}");
            return QuizOutcome::Failed;
        }
    };

    let gid = get_user_gid(uid).unwrap_or(uid);
    let username = uid_to_username(uid).unwrap_or_else(|| uid.to_string());
    let home = uid_to_home_dir(uid).unwrap_or_else(|| format!("/tmp/appcontrol_uid_{uid}"));
    let progress_db = format!("{home}/.local/share/appcontrol/vocab_progress.db");

    let config = vocab_trainer::QuizConfig {
        correct_needed: rule.correct_needed as u32,
        extra_minutes: rule.extra_minutes as u32,
        words_file: rule.words_file.clone(),
        progress_db,
        username,
    };

    unsafe {
        let pid_child = libc::fork();
        if pid_child < 0 {
            eprintln!("appmon vocab_quiz: fork failed");
            return QuizOutcome::Failed;
        }

        if pid_child == 0 {
            // Child: set up display env, drop privileges, run quiz.
            let keys: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
            for k in keys {
                std::env::remove_var(k);
            }
            for (key, val) in &env {
                std::env::set_var(key, val);
            }
            if libc::setresgid(gid, gid, gid) != 0 {
                libc::_exit(2);
            }
            if libc::setresuid(uid, uid, uid) != 0 {
                libc::_exit(2);
            }
            vocab_trainer::run_quiz(config);
            // run_quiz exits the process; this line is unreachable.
            libc::_exit(2);
        }

        // Parent: wait for child.
        let mut status = 0;
        libc::waitpid(pid_child, &mut status, 0);

        if libc::WIFEXITED(status) {
            match libc::WEXITSTATUS(status) {
                0 => QuizOutcome::Earned(rule.extra_minutes * 60),
                1 => QuizOutcome::Quit,
                code => {
                    eprintln!("appmon vocab_quiz: quiz exited with code {code}");
                    QuizOutcome::Failed
                }
            }
        } else {
            eprintln!("appmon vocab_quiz: quiz terminated abnormally");
            QuizOutcome::Failed
        }
    }
}
