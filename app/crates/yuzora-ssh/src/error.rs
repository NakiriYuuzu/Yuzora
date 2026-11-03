//! One error type for the crate: a stable `code` for scripts and other lanes,
//! and a message in the person's language.

use crate::strings::t;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// No such host id, alias or machine.
    UnknownHost,
    /// TCP, proxy or SSH transport failed.
    Connect,
    Timeout,
    /// The host key is not trusted yet and nobody could be asked.
    HostKeyUnknown,
    /// The host key differs from the recorded one and was not accepted.
    HostKeyChanged,
    /// The host key is marked @revoked.
    HostKeyRevoked,
    /// The person declined the host key.
    HostKeyRejected,
    /// Every authentication method failed.
    AuthFailed,
    /// A password, passphrase or answer was needed and nobody could be asked.
    NeedsInput,
    /// The person cancelled a prompt or a transfer.
    Cancelled,
    /// The SSH session is gone.
    Closed,
    /// SFTP refused an operation.
    Sftp,
    /// A local file operation failed.
    Io,
    /// A bad argument (path, name, port).
    Invalid,
}

impl ErrorKind {
    pub fn code(self) -> &'static str {
        match self {
            ErrorKind::UnknownHost => "unknown-host",
            ErrorKind::Connect => "connect",
            ErrorKind::Timeout => "timeout",
            ErrorKind::HostKeyUnknown => "host-key-unknown",
            ErrorKind::HostKeyChanged => "host-key-changed",
            ErrorKind::HostKeyRevoked => "host-key-revoked",
            ErrorKind::HostKeyRejected => "host-key-rejected",
            ErrorKind::AuthFailed => "auth-failed",
            ErrorKind::NeedsInput => "needs-input",
            ErrorKind::Cancelled => "cancelled",
            ErrorKind::Closed => "closed",
            ErrorKind::Sftp => "sftp",
            ErrorKind::Io => "io",
            ErrorKind::Invalid => "invalid",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshError {
    pub kind: ErrorKind,
    pub message: String,
}

impl SshError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }

    /// An error whose message is the crate's `errors.<key>` text.
    pub fn text(kind: ErrorKind, key: &str, vars: &[(&str, String)]) -> Self {
        Self::new(kind, t(&format!("errors.{key}"), vars))
    }

    pub fn code(&self) -> &'static str {
        self.kind.code()
    }

    pub fn io(e: std::io::Error) -> Self {
        Self::new(ErrorKind::Io, e.to_string())
    }

    pub fn sftp(e: russh_sftp::client::error::Error) -> Self {
        Self::new(ErrorKind::Sftp, e.to_string())
    }

    pub fn closed() -> Self {
        Self::text(ErrorKind::Closed, "closed", &[])
    }
}

impl std::fmt::Display for SshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SshError {}

pub type Result<T> = std::result::Result<T, SshError>;
