use std::fs;
use std::path::Path;

pub struct ProcSnapshot {
    pub pid: u32,
    pub uid: u32,
    pub name: String,
    pub cmdline: String,
    pub start_epoch: i64,
}

fn pw_buf_size() -> usize {
    let size = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
    if size > 0 { size as usize } else { 1024 }
}

pub fn uid_to_username(uid: u32) -> Option<String> {
    let mut buf = vec![0 as libc::c_char; pw_buf_size()];
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result: *mut libc::passwd = std::ptr::null_mut();

    let ret = unsafe {
        libc::getpwuid_r(uid, pwd.as_mut_ptr(), buf.as_mut_ptr(), buf.len(), &mut result)
    };

    if ret != 0 || result.is_null() {
        return None;
    }

    let name_ptr = unsafe { pwd.assume_init().pw_name };
    if name_ptr.is_null() {
        return None;
    }

    unsafe { std::ffi::CStr::from_ptr(name_ptr) }
        .to_str()
        .ok()
        .map(|s| s.to_string())
}

pub fn username_to_uid(name: &str) -> Option<u32> {
    let c_name = std::ffi::CString::new(name).ok()?;
    let mut buf = vec![0 as libc::c_char; pw_buf_size()];
    let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result: *mut libc::passwd = std::ptr::null_mut();

    let ret = unsafe {
        libc::getpwnam_r(
            c_name.as_ptr(),
            pwd.as_mut_ptr(),
            buf.as_mut_ptr(),
            buf.len(),
            &mut result,
        )
    };

    if ret != 0 || result.is_null() {
        return None;
    }

    Some(unsafe { pwd.assume_init().pw_uid })
}

pub fn read_boot_time() -> Result<i64, std::io::Error> {
    let content = fs::read_to_string("/proc/stat")?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("btime ") {
            return rest.trim().parse::<i64>().map_err(|e| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, e)
            });
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "btime not found in /proc/stat",
    ))
}

pub fn get_clk_tck() -> i64 {
    let val = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if val == -1 {
        panic!("sysconf(_SC_CLK_TCK) failed");
    }
    val
}

fn read_starttime_ticks(pid: u32) -> Option<u64> {
    let content = fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    // The comm field (field 2) is wrapped in parens and may contain spaces/parens.
    // Find the last ')' to safely skip past it.
    let after_comm = content.rfind(')')?;
    let rest = &content[after_comm + 1..];
    // After the closing paren the fields are:
    // state ppid pgrp session tty_nr tpgid flags minflt cminflt majflt cmajflt
    // utime stime cutime cstime priority nice num_threads itrealvalue starttime
    // That is index 19 (0-based) of the whitespace-split remainder.
    rest.split_whitespace().nth(19)?.parse::<u64>().ok()
}

pub fn read_uid(pid: u32) -> Option<u32> {
    let content = fs::read_to_string(format!("/proc/{}/status", pid)).ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            // Fields: real effective saved filesystem — we want real uid
            return rest.split_whitespace().next()?.parse().ok();
        }
    }
    None
}

pub fn read_comm(pid: u32) -> Option<String> {
    fs::read_to_string(format!("/proc/{}/comm", pid))
        .ok()
        .map(|s| s.trim().to_string())
}

pub fn read_cmdline(pid: u32) -> Option<String> {
    let bytes = fs::read(format!("/proc/{}/cmdline", pid)).ok()?;
    let s = bytes
        .split(|&b| b == 0)
        .filter_map(|chunk| std::str::from_utf8(chunk).ok())
        .collect::<Vec<_>>()
        .join(" ");
    Some(s.trim().to_string())
}

pub fn enumerate_processes(boot_time: i64, clk_tck: i64) -> Vec<ProcSnapshot> {
    let mut snapshots = Vec::new();
    let proc_dir = match fs::read_dir("/proc") {
        Ok(d) => d,
        Err(_) => return snapshots,
    };

    for entry in proc_dir.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let pid: u32 = match name_str.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        if !Path::new(&format!("/proc/{}", pid)).is_dir() {
            continue;
        }

        let ticks = match read_starttime_ticks(pid) {
            Some(t) => t,
            None => continue,
        };
        let start_epoch = boot_time + (ticks as i64 / clk_tck);

        let uid = match read_uid(pid) {
            Some(u) => u,
            None => continue,
        };

        let name = match read_comm(pid) {
            Some(n) => n,
            None => continue,
        };

        let cmdline = read_cmdline(pid).unwrap_or_default();

        snapshots.push(ProcSnapshot {
            pid,
            uid,
            name,
            cmdline,
            start_epoch,
        });
    }

    snapshots
}
