// Windows helper for the update probe (zz-win-update-probe.yml). One file, std only,
// built on the runner with `rustc --edition 2021 -O` (same toolchain as the app).
//
//   winprobe                 (with PROBE_LOG set) the lower-bound chain, see `chain`
//   winprobe ps              every process: pid, parent pid, exe name
//   winprobe procinfo <pid>  image, command line, cwd (string and handle), parent, mitigation
//   winprobe windows <pid>   top-level windows of the process
//   winprobe close <pid>     WM_CLOSE to its visible top-level windows (what taskkill sends)
//   winprobe hold <secs>     sleep (a stand-in process to look at)
//   winprobe ver             Windows version
//
// Every answer is one line of JSON. Nothing here is Yuzora's code: `chain` repeats
// the steps of src/update.rs (find_root, beside, swap, relaunch_expected on Windows)
// so what it learns is a lower bound for the real app.

use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

type Handle = *mut c_void;

const PROCESS_READ_ACCESS: u32 = 0x0400 | 0x0010 | 0x0040; // QUERY_INFORMATION | VM_READ | DUP_HANDLE
const TH32CS_SNAPPROCESS: u32 = 0x2;
const DUPLICATE_SAME_ACCESS: u32 = 0x2;
const WM_CLOSE: u32 = 0x0010;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PROCESS_REDIRECTION_TRUST_POLICY: i32 = 16;

#[repr(C)]
struct ProcessEntry32W {
    size: u32,
    usage: u32,
    pid: u32,
    heap: usize,
    module: u32,
    threads: u32,
    ppid: u32,
    pri: i32,
    flags: u32,
    exe: [u16; 260],
}

#[repr(C)]
struct ProcessBasicInformation {
    exit_status: i32,
    peb: usize,
    affinity: usize,
    base_priority: i32,
    pid: usize,
    ppid: usize,
}

#[repr(C)]
struct OsVersionInfo {
    size: u32,
    major: u32,
    minor: u32,
    build: u32,
    platform: u32,
    csd: [u16; 128],
}

#[link(name = "kernel32")]
extern "system" {
    fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
    fn CloseHandle(h: Handle) -> i32;
    fn GetCurrentProcess() -> Handle;
    fn GetLastError() -> u32;
    fn ReadProcessMemory(h: Handle, base: usize, buf: *mut c_void, size: usize, read: *mut usize) -> i32;
    fn DuplicateHandle(from: Handle, src: Handle, to: Handle, dst: *mut Handle, access: u32, inherit: i32, options: u32) -> i32;
    fn GetFinalPathNameByHandleW(h: Handle, buf: *mut u16, len: u32, flags: u32) -> u32;
    fn QueryFullProcessImageNameW(h: Handle, flags: u32, buf: *mut u16, size: *mut u32) -> i32;
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> Handle;
    fn Process32FirstW(snap: Handle, entry: *mut ProcessEntry32W) -> i32;
    fn Process32NextW(snap: Handle, entry: *mut ProcessEntry32W) -> i32;
    fn GetProcessMitigationPolicy(h: Handle, policy: i32, buf: *mut c_void, len: usize) -> i32;
}

#[link(name = "ntdll")]
extern "system" {
    fn NtQueryInformationProcess(h: Handle, class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
    fn RtlGetVersion(info: *mut OsVersionInfo) -> i32;
}

#[link(name = "user32")]
extern "system" {
    fn EnumWindows(cb: extern "system" fn(Handle, isize) -> i32, lparam: isize) -> i32;
    fn GetWindowThreadProcessId(hwnd: Handle, pid: *mut u32) -> u32;
    fn IsWindowVisible(hwnd: Handle) -> i32;
    fn GetWindowTextW(hwnd: Handle, buf: *mut u16, max: i32) -> i32;
    fn GetClassNameW(hwnd: Handle, buf: *mut u16, max: i32) -> i32;
    fn PostMessageW(hwnd: Handle, msg: u32, w: usize, l: isize) -> i32;
}

// ---- JSON by hand ---------------------------------------------------------

fn js(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn jopt(s: &Option<String>) -> String {
    s.as_deref().map_or("null".to_owned(), js)
}

fn obj(fields: &[(&str, String)]) -> String {
    let body: Vec<String> = fields.iter().map(|(k, v)| format!("{}:{}", js(k), v)).collect();
    format!("{{{}}}", body.join(","))
}

fn io_err(e: &std::io::Error) -> String {
    js(&e.to_string())
}

fn result_json<T>(r: &std::io::Result<T>) -> String {
    match r {
        Ok(_) => obj(&[("ok", "true".into())]),
        Err(e) => obj(&[("ok", "false".into()), ("error", io_err(e)), ("code", e.raw_os_error().map_or("null".into(), |c| c.to_string()))]),
    }
}

// ---- processes ------------------------------------------------------------

struct Proc {
    pid: u32,
    ppid: u32,
    name: String,
}

fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn processes() -> Vec<Proc> {
    let mut out = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap as isize == -1 {
            return out;
        }
        let mut entry: ProcessEntry32W = std::mem::zeroed();
        entry.size = std::mem::size_of::<ProcessEntry32W>() as u32;
        let mut more = Process32FirstW(snap, &mut entry);
        while more != 0 {
            out.push(Proc { pid: entry.pid, ppid: entry.ppid, name: wide_to_string(&entry.exe) });
            more = Process32NextW(snap, &mut entry);
        }
        CloseHandle(snap);
    }
    out
}

fn read_bytes(h: Handle, addr: usize, len: usize) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; len];
    let mut got = 0usize;
    let ok = unsafe { ReadProcessMemory(h, addr, buf.as_mut_ptr() as *mut c_void, len, &mut got) };
    (ok != 0 && got == len).then_some(buf)
}

fn read_usize(h: Handle, addr: usize) -> Option<usize> {
    let b = read_bytes(h, addr, 8)?;
    Some(usize::from_le_bytes(b.try_into().ok()?))
}

/// A UNICODE_STRING at `addr` in the other process (x64 layout).
fn read_unicode_string(h: Handle, addr: usize) -> Option<String> {
    let head = read_bytes(h, addr, 16)?;
    let len = u16::from_le_bytes([head[0], head[1]]) as usize;
    let buffer = usize::from_le_bytes(head[8..16].try_into().ok()?);
    if len == 0 {
        return Some(String::new());
    }
    let raw = read_bytes(h, buffer, len)?;
    let wide: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Some(String::from_utf16_lossy(&wide))
}

fn final_path(h: Handle) -> Option<String> {
    let mut buf = vec![0u16; 2048];
    let n = unsafe { GetFinalPathNameByHandleW(h, buf.as_mut_ptr(), buf.len() as u32, 0) } as usize;
    (n > 0 && n < buf.len()).then(|| String::from_utf16_lossy(&buf[..n]))
}

struct PebInfo {
    image: Option<String>,
    cmdline: Option<String>,
    cwd_dos: Option<String>,
    /// Where the cwd handle really points: the PEB's string goes stale when the folder is renamed.
    cwd_final: Option<String>,
}

fn peb_info(h: Handle) -> Option<PebInfo> {
    unsafe {
        let mut pbi: ProcessBasicInformation = std::mem::zeroed();
        let mut ret = 0u32;
        let status = NtQueryInformationProcess(h, 0, &mut pbi as *mut _ as *mut c_void, std::mem::size_of::<ProcessBasicInformation>() as u32, &mut ret);
        if status != 0 || pbi.peb == 0 {
            return None;
        }
        // x64: PEB.ProcessParameters at 0x20; in RTL_USER_PROCESS_PARAMETERS CurrentDirectory
        // (DosPath UNICODE_STRING, then Handle) at 0x38, ImagePathName at 0x60, CommandLine at 0x70.
        let params = read_usize(h, pbi.peb + 0x20)?;
        let cwd_dos = read_unicode_string(h, params + 0x38);
        let remote_handle = read_usize(h, params + 0x48);
        let cwd_final = remote_handle.filter(|&v| v != 0).and_then(|v| {
            let mut dup: Handle = std::ptr::null_mut();
            let ok = DuplicateHandle(h, v as Handle, GetCurrentProcess(), &mut dup, 0, 0, DUPLICATE_SAME_ACCESS);
            if ok == 0 {
                return None;
            }
            let path = final_path(dup);
            CloseHandle(dup);
            path
        });
        Some(PebInfo { image: read_unicode_string(h, params + 0x60), cmdline: read_unicode_string(h, params + 0x70), cwd_dos, cwd_final })
    }
}

fn peb_json(info: &Option<PebInfo>) -> String {
    match info {
        None => "null".into(),
        Some(i) => obj(&[("image", jopt(&i.image)), ("cmdline", jopt(&i.cmdline)), ("cwd_dos", jopt(&i.cwd_dos)), ("cwd_final", jopt(&i.cwd_final))]),
    }
}

fn redirection_trust(h: Handle) -> String {
    let mut flags: u32 = 0;
    let ok = unsafe { GetProcessMitigationPolicy(h, PROCESS_REDIRECTION_TRUST_POLICY, &mut flags as *mut u32 as *mut c_void, 4) };
    if ok == 0 {
        let err = unsafe { GetLastError() };
        return obj(&[("measured", "false".into()), ("win32_error", err.to_string())]);
    }
    obj(&[("measured", "true".into()), ("flags", flags.to_string()), ("enforce", (flags & 1 != 0).to_string()), ("audit", (flags & 2 != 0).to_string())])
}

fn image_path(h: Handle) -> Option<String> {
    let mut buf = vec![0u16; 2048];
    let mut size = buf.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size) };
    (ok != 0).then(|| String::from_utf16_lossy(&buf[..size as usize]))
}

fn procinfo(pid: u32) -> String {
    let all = processes();
    let me = all.iter().find(|p| p.pid == pid);
    let parent = me.and_then(|m| all.iter().find(|p| p.pid == m.ppid));
    let h = unsafe { OpenProcess(PROCESS_READ_ACCESS, 0, pid) };
    if h.is_null() {
        let err = unsafe { GetLastError() };
        return obj(&[("pid", pid.to_string()), ("alive", me.is_some().to_string()), ("error", js(&format!("OpenProcess failed: {err}")))]);
    }
    let peb = peb_info(h);
    let json = obj(&[
        ("pid", pid.to_string()),
        ("alive", me.is_some().to_string()),
        ("ppid", me.map_or("null".into(), |m| m.ppid.to_string())),
        ("parent_name", parent.map_or("null".into(), |p| js(&p.name))),
        ("image_query", jopt(&image_path(h))),
        ("peb", peb_json(&peb)),
        ("redirection_trust", redirection_trust(h)),
    ]);
    unsafe { CloseHandle(h) };
    json
}

// ---- windows --------------------------------------------------------------

struct Found {
    pid: u32,
    list: Vec<(usize, bool, String, String)>,
}

extern "system" fn each_window(hwnd: Handle, lparam: isize) -> i32 {
    let found = unsafe { &mut *(lparam as *mut Found) };
    let mut owner = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
    if owner == found.pid {
        let mut title = vec![0u16; 256];
        let mut class = vec![0u16; 256];
        let t = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), 256) }.max(0) as usize;
        let c = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), 256) }.max(0) as usize;
        let visible = unsafe { IsWindowVisible(hwnd) } != 0;
        found.list.push((hwnd as usize, visible, String::from_utf16_lossy(&class[..c]), String::from_utf16_lossy(&title[..t])));
    }
    1
}

fn windows_of(pid: u32) -> Vec<(usize, bool, String, String)> {
    let mut found = Found { pid, list: Vec::new() };
    unsafe { EnumWindows(each_window, &mut found as *mut Found as isize) };
    found.list
}

fn windows_json(list: &[(usize, bool, String, String)]) -> String {
    let items: Vec<String> = list
        .iter()
        .map(|(h, visible, class, title)| obj(&[("hwnd", h.to_string()), ("visible", visible.to_string()), ("class", js(class)), ("title", js(title))]))
        .collect();
    format!("[{}]", items.join(","))
}

// ---- the lower-bound chain ------------------------------------------------

fn log_event(kind: &str, fields: &[(&str, String)]) {
    use std::io::Write;
    let Some(path) = std::env::var_os("PROBE_LOG") else { return };
    let mut all = vec![("ev", js(kind)), ("pid", std::process::id().to_string()), ("ms", unix_ms().to_string())];
    all.extend(fields.iter().map(|(k, v)| (*k, v.clone())));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{}", obj(&all));
    }
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// src/update.rs `beside`.
fn beside(root: &Path, suffix: &str) -> PathBuf {
    let mut name = root.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    root.with_file_name(name)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn list_dir(dir: &Path) -> String {
    match std::fs::read_dir(dir) {
        Ok(rd) => {
            let names: Vec<String> = rd.flatten().map(|e| js(&e.file_name().to_string_lossy())).collect();
            format!("[{}]", names.join(","))
        }
        Err(e) => js(&e.to_string()),
    }
}

/// A folder that is not there counts as removed (the app ignores that error too).
fn remove_dir_all_or_absent(dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// One round of an update: look at this process, stage a copy of the folder, swap it in
/// (src/update.rs `swap`), reopen the app the way `relaunch_expected` does on Windows.
fn chain() -> i32 {
    let round: u32 = std::env::var("PROBE_ROUND").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let rounds: u32 = std::env::var("PROBE_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    // PROBE_CHDIR: what a fix that moves the app's own cwd away at startup would do (first round only,
    // the reopened programs inherit what this one has by then).
    if round == 1 {
        if let Some(dir) = std::env::var_os("PROBE_CHDIR") {
            let before = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let moved = std::env::set_current_dir(&dir);
            log_event("chdir", &[("before", js(&before)), ("to", js(&dir.to_string_lossy())), ("result", result_json(&moved))]);
        }
    }
    // find_root: the canonical path of the running program, `\\?\C:\...` on Windows.
    let exe = match std::env::current_exe().and_then(|p| p.canonicalize()) {
        Ok(p) => p,
        Err(e) => {
            log_event("error", &[("what", js("current_exe")), ("error", io_err(&e))]);
            return 2;
        }
    };
    let root = exe.parent().unwrap().to_path_buf();
    let me = unsafe { GetCurrentProcess() };
    let all = processes();
    let ppid = all.iter().find(|p| p.pid == std::process::id()).map(|p| p.ppid);
    let parent = ppid.and_then(|pp| all.iter().find(|p| p.pid == pp)).map(|p| p.name.clone());
    log_event(
        "start",
        &[
            ("round", round.to_string()),
            ("rounds", rounds.to_string()),
            ("exe", js(&exe.to_string_lossy())),
            ("cwd_std", js(&std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default())),
            ("ppid", ppid.map_or("null".into(), |p| p.to_string())),
            ("parent_name", jopt(&parent)),
            ("peb", peb_json(&peb_info(me))),
            ("old_exists", beside(&root, ".old").exists().to_string()),
            ("old_listing", list_dir(&beside(&root, ".old"))),
        ],
    );
    let old = beside(&root, ".old");
    if round >= rounds {
        let cleanup = remove_dir_all_or_absent(&old);
        log_event("final_cleanup", &[("remove_old", result_json(&cleanup)), ("old_still_there", old.exists().to_string())]);
        log_event("done", &[]);
        return 0;
    }

    let staged = beside(&root, ".new");
    let _ = std::fs::remove_dir_all(&staged);
    let copied = copy_dir(&root, &staged);
    log_event("stage", &[("copy", result_json(&copied))]);
    if copied.is_err() {
        return 2;
    }

    let remove_old = remove_dir_all_or_absent(&old);
    log_event("swap_remove_old", &[("result", result_json(&remove_old)), ("old_still_there", old.exists().to_string()), ("old_listing", list_dir(&old))]);
    let aside = std::fs::rename(&root, &old);
    log_event("swap_root_to_old", &[("result", result_json(&aside))]);
    if aside.is_err() {
        log_event("swap_failed", &[("at", js("rename root -> old"))]);
        return 3;
    }
    let placed = std::fs::rename(&staged, &root);
    log_event("swap_new_to_root", &[("result", result_json(&placed))]);
    if placed.is_err() {
        let rollback = std::fs::rename(&old, &root);
        log_event("swap_failed", &[("at", js("rename new -> root")), ("rollback", result_json(&rollback))]);
        return 3;
    }

    let mut new_exe = root.join(exe.file_name().unwrap()).to_string_lossy().into_owned();
    // The app hands PowerShell the `\\?\` path as it is; PROBE_RELAUNCH=plain drops the prefix to tell
    // a relaunch that fails on the prefix from a swap that fails.
    if std::env::var("PROBE_RELAUNCH").as_deref() == Ok("plain") {
        if let Some(plain) = new_exe.strip_prefix(r"\\?\") {
            new_exe = plain.to_owned();
        }
    }
    let new_exe = new_exe.replace('\'', "''");
    let script = format!("Wait-Process -Id {} -ErrorAction SilentlyContinue; Start-Process '{new_exe}'", std::process::id());
    let spawned = Command::new("powershell")
        .env("PROBE_ROUND", (round + 1).to_string())
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    log_event("relaunch", &[("script", js(&script)), ("result", result_json(&spawned))]);
    0
}

fn hold(secs: u64) {
    std::thread::sleep(std::time::Duration::from_secs(secs));
}

fn pid_arg(args: &[String]) -> u32 {
    args.get(2).and_then(|v| v.parse().ok()).unwrap_or(0)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let code = match args.get(1).map(String::as_str) {
        None if std::env::var_os("PROBE_LOG").is_some() => chain(),
        Some("ps") => {
            for p in processes() {
                println!("{}", obj(&[("pid", p.pid.to_string()), ("ppid", p.ppid.to_string()), ("name", js(&p.name))]));
            }
            0
        }
        Some("procinfo") => {
            println!("{}", procinfo(pid_arg(&args)));
            0
        }
        Some("windows") => {
            println!("{}", windows_json(&windows_of(pid_arg(&args))));
            0
        }
        Some("close") => {
            let pid = pid_arg(&args);
            let list = windows_of(pid);
            let mut posted = 0;
            for (hwnd, visible, _, _) in &list {
                if *visible && unsafe { PostMessageW(*hwnd as Handle, WM_CLOSE, 0, 0) } != 0 {
                    posted += 1;
                }
            }
            println!("{}", obj(&[("windows", list.len().to_string()), ("posted", posted.to_string())]));
            0
        }
        Some("hold") => {
            hold(args.get(2).and_then(|v| v.parse().ok()).unwrap_or(30));
            0
        }
        Some("ver") => {
            let mut v: OsVersionInfo = unsafe { std::mem::zeroed() };
            v.size = std::mem::size_of::<OsVersionInfo>() as u32;
            unsafe { RtlGetVersion(&mut v) };
            let own = redirection_trust(unsafe { GetCurrentProcess() });
            println!("{}", obj(&[("major", v.major.to_string()), ("minor", v.minor.to_string()), ("build", v.build.to_string()), ("own_redirection_trust", own)]));
            0
        }
        _ => {
            eprintln!("winprobe: ps | procinfo <pid> | windows <pid> | close <pid> | hold <secs> | ver; with PROBE_LOG set and no argument: chain");
            64
        }
    };
    std::process::exit(code);
}
