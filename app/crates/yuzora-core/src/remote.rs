//! Running one program here or on another machine, with a timeout the caller
//! picks. Another machine goes through herdr-core: `hc exec --json --on
//! <machine> --timeout <s> [--cwd <dir>] -- <program> <args…>`, which prints
//! one JSON object with the exit code and base64 stdout/stderr (the same call
//! the app's `exec({on})` makes, there fixed at 30 s and 8 MiB).
//!
//! Blocking: call it from a worker thread (the `yuzora:core` `exec` does).

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::Value;

/// The default limit, as Herdr's own `exec`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Bytes kept per stream; the rest is drained and dropped.
pub const DEFAULT_MAX_OUTPUT: u64 = 8 << 20;
/// How long past the remote timeout the local `hc` may take to answer.
const REMOTE_GRACE: Duration = Duration::from_secs(5);

static HC: OnceLock<PathBuf> = OnceLock::new();

/// The herdr-core the app found (`hc` on PATH until then).
pub fn set_hc_program(path: PathBuf) {
    let _ = HC.set(path);
}

pub fn hc_program() -> PathBuf {
    HC.get().cloned().unwrap_or_else(|| PathBuf::from("hc"))
}

/// One program to run.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    /// A machine name from `hc machine ls`; `None` runs it here.
    pub on: Option<String>,
    pub timeout: Duration,
    pub max_output: u64,
}

impl ExecRequest {
    pub fn new(program: impl Into<String>, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            cwd: None,
            on: None,
            timeout: DEFAULT_TIMEOUT,
            max_output: DEFAULT_MAX_OUTPUT,
        }
    }

    pub fn on(mut self, machine: Option<impl Into<String>>) -> Self {
        self.on = machine.map(Into::into).filter(|m: &String| !m.is_empty());
        self
    }

    pub fn cwd(mut self, cwd: Option<impl Into<String>>) -> Self {
        self.cwd = cwd.map(Into::into);
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn max_output(mut self, bytes: u64) -> Self {
        self.max_output = bytes;
        self
    }
}

/// What the program printed and how it ended.
#[derive(Clone, Debug, PartialEq)]
pub struct ExecOutput {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ExecOutput {
    pub fn stdout_text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExecError {
    /// The program (or `hc`) could not start.
    Spawn(String),
    TimedOut { seconds: f64, machine: Option<String> },
    /// herdr-core refused or could not reach the machine.
    Remote(String),
    /// `hc exec` answered with something that isn't its JSON.
    Protocol(String),
}

impl std::fmt::Display for ExecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecError::Spawn(e) | ExecError::Remote(e) | ExecError::Protocol(e) => f.write_str(e),
            ExecError::TimedOut { seconds, machine: Some(m) } => write!(f, "took longer than {seconds} s on {m} and was stopped"),
            ExecError::TimedOut { seconds, machine: None } => write!(f, "took longer than {seconds} s and was stopped"),
        }
    }
}

impl std::error::Error for ExecError {}

/// The `hc` arguments for a remote request (`None` when it runs here).
pub fn hc_exec_args(req: &ExecRequest) -> Option<Vec<String>> {
    let machine = req.on.as_ref()?;
    // hc takes whole seconds; never round a short timeout down to "none".
    let secs = req.timeout.as_secs_f64().ceil().max(1.0) as u64;
    let mut args = vec!["exec".into(), "--json".into(), "--on".into(), machine.clone(), "--timeout".into(), secs.to_string()];
    if let Some(cwd) = &req.cwd {
        args.extend(["--cwd".into(), cwd.clone()]);
    }
    args.push("--".into());
    args.push(req.program.clone());
    args.extend(req.args.iter().cloned());
    Some(args)
}

/// Reads `hc exec --json`'s answer (on stdout, or stderr for its own errors).
pub fn parse_hc_exec(stdout: &str, stderr: &str, req: &ExecRequest) -> Result<ExecOutput, ExecError> {
    let v: Value = serde_json::from_str(stdout.trim())
        .or_else(|_| serde_json::from_str(stderr.trim()))
        .map_err(|_| ExecError::Protocol(format!("hc exec: {}", stderr.trim())))?;
    if let Some(e) = v.get("error") {
        let message = e.get("message").and_then(Value::as_str).unwrap_or("hc exec failed").to_owned();
        return Err(ExecError::Remote(message));
    }
    if v.get("timed_out").and_then(Value::as_bool) == Some(true) {
        return Err(ExecError::TimedOut { seconds: req.timeout.as_secs_f64(), machine: req.on.clone() });
    }
    let bytes = |k: &str| {
        base64::engine::general_purpose::STANDARD
            .decode(v.get(k).and_then(Value::as_str).unwrap_or_default())
            .map_err(|e| ExecError::Protocol(format!("hc exec {k}: {e}")))
    };
    let code = v.get("code").and_then(Value::as_i64).map_or(-1, |c| c as i32);
    Ok(ExecOutput { code, stdout: bytes("stdout")?, stderr: bytes("stderr")? })
}

/// Runs the request and waits for it (blocking).
pub fn run(req: &ExecRequest) -> Result<ExecOutput, ExecError> {
    match hc_exec_args(req) {
        Some(args) => {
            let local = ExecRequest {
                program: hc_program().to_string_lossy().into_owned(),
                args,
                cwd: None,
                on: None,
                timeout: req.timeout + REMOTE_GRACE,
                max_output: req.max_output.saturating_mul(2).saturating_add(4096),
            };
            let out = run_local(&local)?;
            parse_hc_exec(&out.stdout_text(), &out.stderr_text(), req)
        }
        None => run_local(req),
    }
}

fn run_local(req: &ExecRequest) -> Result<ExecOutput, ExecError> {
    let mut cmd = Command::new(&req.program);
    cmd.args(&req.args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.current_dir(req.cwd.as_ref().map(PathBuf::from).unwrap_or_else(crate::paths::home_dir));
    let mut child = cmd.spawn().map_err(|e| ExecError::Spawn(format!("cannot run {}: {e}", req.program)))?;
    let limit = req.max_output;
    let drain = |r: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut r = r;
            let mut buf = Vec::new();
            let _ = (&mut r).take(limit).read_to_end(&mut buf);
            let _ = std::io::copy(&mut r, &mut std::io::sink());
            buf
        })
    };
    let out = drain(Box::new(child.stdout.take().expect("piped stdout")));
    let err = drain(Box::new(child.stderr.take().expect("piped stderr")));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() > req.timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ExecError::TimedOut { seconds: req.timeout.as_secs_f64(), machine: None });
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(ExecError::Spawn(e.to_string())),
        }
    };
    Ok(ExecOutput { code: status.code().unwrap_or(-1), stdout: out.join().unwrap_or_default(), stderr: err.join().unwrap_or_default() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_requests_become_hc_exec_with_their_timeout() {
        let req = ExecRequest::new("git", ["status", "--short"]).on(Some("lab-01")).cwd(Some("/srv/repo")).timeout(Duration::from_millis(2500));
        assert_eq!(
            hc_exec_args(&req).unwrap(),
            ["exec", "--json", "--on", "lab-01", "--timeout", "3", "--cwd", "/srv/repo", "--", "git", "status", "--short"]
        );
        assert_eq!(hc_exec_args(&ExecRequest::new("ls", Vec::<String>::new())), None);
        assert_eq!(hc_exec_args(&ExecRequest::new("ls", Vec::<String>::new()).on(Some(""))), None);
        let tiny = ExecRequest::new("true", Vec::<String>::new()).on(Some("m")).timeout(Duration::from_millis(1));
        assert_eq!(hc_exec_args(&tiny).unwrap()[5], "1");
    }

    #[test]
    fn hc_exec_answers_are_decoded() {
        let req = ExecRequest::new("echo", ["hi"]).on(Some("vps"));
        let ok = parse_hc_exec(r#"{"code":0,"stdout":"aGkK","stderr":""}"#, "", &req).unwrap();
        assert_eq!((ok.code, ok.stdout_text().as_str()), (0, "hi\n"));
        let refused = parse_hc_exec("", r#"{"error":{"message":"no machine vps"}}"#, &req);
        assert_eq!(refused, Err(ExecError::Remote("no machine vps".into())));
        let slow = parse_hc_exec(r#"{"timed_out":true,"code":null}"#, "", &req).unwrap_err();
        assert!(slow.to_string().contains("on vps"));
        assert!(matches!(parse_hc_exec("nonsense", "boom", &req), Err(ExecError::Protocol(_))));
    }

    #[cfg(unix)]
    #[test]
    fn local_runs_capture_output_and_stop_at_the_timeout() {
        let out = run(&ExecRequest::new("sh", ["-c", "printf out; printf err >&2; exit 3"])).unwrap();
        assert_eq!((out.code, out.stdout_text().as_str(), out.stderr_text().as_str()), (3, "out", "err"));
        let capped = run(&ExecRequest::new("sh", ["-c", "printf 0123456789"]).max_output(4)).unwrap();
        assert_eq!(capped.stdout_text(), "0123");
        let started = Instant::now();
        let slow = run(&ExecRequest::new("sleep", ["5"]).timeout(Duration::from_millis(200)));
        assert!(matches!(slow, Err(ExecError::TimedOut { .. })));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(matches!(run(&ExecRequest::new("yuzora-no-such-program", Vec::<String>::new())), Err(ExecError::Spawn(_))));
    }
}
