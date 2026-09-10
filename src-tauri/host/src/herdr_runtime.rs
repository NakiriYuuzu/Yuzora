//! Read-only compatibility evidence shared by native and remote configuration.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub fn inspect_local(binary: std::path::PathBuf) -> Result<RuntimeBinaryCheck, String> {
    use crate::herdr_backend::HerdrMetadata;
    let manager = crate::herdr_service::HerdrManager::with_binary(binary.clone());
    let schema = manager.metadata(HerdrMetadata::Schema, Some("default"))?;
    let names = session_names(&manager.metadata(HerdrMetadata::Sessions, Some("default"))?)?;
    let statuses = names
        .into_iter()
        .map(|name| {
            let status = manager.metadata(HerdrMetadata::Status, Some(&name))?;
            Ok((name, status))
        })
        .collect::<Result<Vec<_>, String>>()?;
    inspect_documents(binary.to_string_lossy().into_owned(), schema, statuses)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSessionCheck {
    pub name: String,
    pub running: bool,
    pub server_version: Option<String>,
    pub server_protocol: Option<u64>,
    pub compatible: Option<bool>,
    pub socket: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeBinaryCheck {
    pub binary: String,
    pub reported_binary: Option<String>,
    pub client_version: String,
    pub client_protocol: u64,
    pub schema_protocol: u64,
    pub missing_methods: Vec<String>,
    pub sessions: Vec<RuntimeSessionCheck>,
    pub can_apply: bool,
}

impl RuntimeBinaryCheck {
    pub fn require_compatible(&self) -> Result<(), String> {
        if self.can_apply {
            return Ok(());
        }
        let affected = self
            .sessions
            .iter()
            .filter(|s| s.running && s.compatible != Some(true))
            .map(|s| {
                format!(
                    "{}: server {} / protocol {:?} / socket {}",
                    s.name,
                    s.server_version.as_deref().unwrap_or("unknown"),
                    s.server_protocol,
                    s.socket.as_deref().unwrap_or("unknown")
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        Err(format!("runtime-incompatible: client {} / protocol {} at {}; schema protocol {}; missing methods {:?}; {}. Preserve running Sessions and select a compatible client.", self.client_version, self.client_protocol, self.binary, self.schema_protocol, self.missing_methods, affected))
    }
}

pub fn session_names(value: &Value) -> Result<Vec<String>, String> {
    let rows = value
        .get("sessions")
        .and_then(Value::as_array)
        .ok_or("invalid-runtime-session-list")?;
    if rows.len() > 128 {
        return Err("too-many-runtime-sessions".into());
    }
    let mut names = vec!["default".to_string()];
    for row in rows {
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .ok_or("invalid-runtime-session-name")?;
        if name.is_empty() || name.len() > 128 || name.contains('\0') {
            return Err("invalid-runtime-session-name".into());
        }
        if row.get("running").and_then(Value::as_bool) == Some(true)
            && !names.iter().any(|n| n == name)
        {
            names.push(name.to_string());
        }
    }
    Ok(names)
}

pub fn inspect_documents(
    binary: String,
    schema: Value,
    statuses: Vec<(String, Value)>,
) -> Result<RuntimeBinaryCheck, String> {
    let first = &statuses.first().ok_or("runtime-status-missing")?.1;
    let client = first
        .get("client")
        .ok_or("runtime-client-identity-missing")?;
    let client_version = client
        .get("version")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("runtime-client-version-missing")?
        .to_string();
    let client_protocol = client
        .get("protocol")
        .and_then(Value::as_u64)
        .ok_or("runtime-client-protocol-missing")?;
    let schema_protocol = schema
        .get("protocol")
        .and_then(Value::as_u64)
        .ok_or("runtime-schema-protocol-missing")?;
    let reported_binary = client
        .get("binary")
        .and_then(Value::as_str)
        .map(str::to_string);
    let methods = crate::herdr_service::collect_schema_methods(&schema);
    let missing_methods: Vec<String> = ["session.snapshot", "events.subscribe", "tab.create"]
        .into_iter()
        .filter(|method| !methods.iter().any(|m| m == method))
        .map(str::to_string)
        .collect();
    let mut sessions = Vec::new();
    for (name, status) in statuses {
        if status.pointer("/client/version").and_then(Value::as_str) != Some(&client_version)
            || status.pointer("/client/protocol").and_then(Value::as_u64) != Some(client_protocol)
            || status.pointer("/client/binary").and_then(Value::as_str)
                != reported_binary.as_deref()
        {
            return Err("runtime-client-changed-during-check".into());
        }
        let server = status
            .get("server")
            .ok_or("runtime-server-status-missing")?;
        let running = server
            .get("running")
            .and_then(Value::as_bool)
            .ok_or("runtime-server-running-missing")?;
        let server_protocol = server.get("protocol").and_then(Value::as_u64);
        let compatible = if running {
            Some(
                server.get("compatible").and_then(Value::as_bool) == Some(true)
                    && server_protocol == Some(client_protocol),
            )
        } else {
            None
        };
        sessions.push(RuntimeSessionCheck {
            name,
            running,
            compatible,
            server_protocol,
            server_version: server
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string),
            socket: server
                .get("socket")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }
    let can_apply = client_protocol == schema_protocol
        && missing_methods.is_empty()
        && sessions
            .iter()
            .all(|s| !s.running || s.compatible == Some(true));
    Ok(RuntimeBinaryCheck {
        binary,
        reported_binary,
        client_version,
        client_protocol,
        schema_protocol,
        missing_methods,
        sessions,
        can_apply,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn schema() -> Value {
        json!({"protocol":22,"methods":["session.snapshot","events.subscribe","tab.create"]})
    }
    fn status(protocol: u64, compatible: Value) -> Value {
        json!({"client":{"version":"0.9.0","protocol":22,"binary":"/chosen/herdr"},"server":{"running":true,"version":"0.9.0","protocol":protocol,"compatible":compatible,"socket":"/original.sock"}})
    }
    #[test]
    fn checks_every_running_session_and_does_not_substitute_endpoint_compatibility() {
        let mut incompatible = status(20, json!(false));
        incompatible["server"]["endpoint_compatible"] = json!(true);
        let check = inspect_documents(
            "/chosen/herdr".into(),
            schema(),
            vec![
                ("default".into(), status(22, json!(true))),
                ("work".into(), incompatible),
            ],
        )
        .unwrap();
        assert!(!check.can_apply);
        assert!(check
            .require_compatible()
            .unwrap_err()
            .contains("work: server"));
    }
    #[test]
    fn accepts_matching_client_and_server_without_changing_the_socket() {
        let check = inspect_documents(
            "/chosen/herdr".into(),
            schema(),
            vec![("default".into(), status(22, json!(true)))],
        )
        .unwrap();
        assert!(check.can_apply);
        assert_eq!(check.sessions[0].socket.as_deref(), Some("/original.sock"));
    }
    #[test]
    fn rejects_unknown_compatibility_and_schema_drift() {
        assert!(
            !inspect_documents(
                "/chosen/herdr".into(),
                schema(),
                vec![("default".into(), status(22, Value::Null))]
            )
            .unwrap()
            .can_apply
        );
        let mut wrong = schema();
        wrong["protocol"] = json!(20);
        assert!(
            !inspect_documents(
                "/chosen/herdr".into(),
                wrong,
                vec![("default".into(), status(22, json!(true)))]
            )
            .unwrap()
            .can_apply
        );
    }
    #[test]
    fn reports_the_actual_old_selected_client_even_when_server_is_new() {
        let mut old = status(22, json!(false));
        old["client"]["version"] = json!("0.8.2");
        old["client"]["protocol"] = json!(20);
        let mut old_schema = schema();
        old_schema["protocol"] = json!(20);
        let check = inspect_documents(
            "/old/managed/herdr".into(),
            old_schema,
            vec![("default".into(), old)],
        )
        .unwrap();
        assert_eq!(check.client_version, "0.8.2");
        assert!(!check.can_apply);
        assert_eq!(check.sessions[0].server_protocol, Some(22));
    }
}
