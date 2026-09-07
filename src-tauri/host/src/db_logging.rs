//! The shared core emits structured diagnostics through the desktop sink.
pub use crate::log_event::mask_url_userinfo;
use crate::log_event::LogEvent;
static SINK: std::sync::OnceLock<fn(LogEvent)> = std::sync::OnceLock::new();
pub fn set_sink(sink: fn(LogEvent)) {
    let _ = SINK.set(sink);
}
pub(crate) fn write_global(event: LogEvent) {
    if let Some(sink) = SINK.get() {
        sink(event);
    }
}
pub fn connect_failure_event(
    source: &str,
    host: &str,
    port: u16,
    user: &str,
    reason: &str,
) -> LogEvent {
    let host = mask_url_userinfo(host);
    let reason = mask_url_userinfo(reason);
    LogEvent {
        level: "warn".to_string(),
        kind: "debug".to_string(),
        source: source.to_string(),
        workspace_path: None,
        event: "connect_failed".to_string(),
        message: format!("{source} connection to {user}@{host}:{port} failed: {reason}"),
        metadata: serde_json::json!({ "host": host, "port": port, "user": user }),
    }
}
