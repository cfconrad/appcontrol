use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use log;

/// Number of times a uid can click "Ok" before only "Close Application" remains.
const WARN_LIMIT: u32 = 3;

/// Seconds to wait before re-showing the popup after "Ok" is clicked.
const REPEAT_SECS: u64 = 60;

/// Per-uid notification counts (how many times the popup has been shown).
static COUNTS: LazyLock<Mutex<HashMap<u32, u32>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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
    loop {
        log::debug!("Run notif_looop for uid:{} pid:{}", uid, pid);
        if !is_process_running(pid) {
            break;
        }

        let count = {
            let mut map = COUNTS.lock().unwrap();
            let c = map.entry(uid).or_insert(0);
            *c += 1;
            *c
        };

        if count >= WARN_LIMIT {
            kill_processe(pid);
            break;
        }

        match show_dialog(uid, &message, false) {
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
}

/// Spawn a `zenity` dialog as the target user on their display.
fn show_dialog(uid: u32, message: &str) -> DialogResult {
    let env = match find_user_display_env(uid) {
        Some(e) => e,
        None => {
            eprintln!("appmon notify: cannot find display session for uid {uid}");
            return DialogResult::Failed;
        }
    };

    let gid = get_user_gid(uid).unwrap_or(uid);

    let mut cmd = std::process::Command::new("xpopup");
    cmd.env_clear();

    // Forward only the vars zenity needs.
    for key in &[
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "LANG",
        "LC_ALL",
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "XAUTHORITY",
        "DBUS_SESSION_BUS_ADDRESS",
        "XDG_RUNTIME_DIR",
        "XDG_SESSION_TYPE",
    ] {
        if let Some(val) = env.get(*key) {
            cmd.env(key, val);
        }
    }

    cmd.args([
        message.to_string(),
    ]);

    // Drop privileges to the target user so X11/Wayland auth works.
    use std::os::unix::process::CommandExt;
    cmd.uid(uid).gid(gid);

    match cmd.status() {
        Err(e) => {
            eprintln!("appmon notify: failed to launch zenity: {e}");
            DialogResult::Failed
        }
        Ok(status) => {
            if status.success() {
                DialogResult::Ok
            } else {
                DialogResult::CloseApplication
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
        libc::kill(pid.try_into().unwrap() , libc::SIGTERM);
    }
}
