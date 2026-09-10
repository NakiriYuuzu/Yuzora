use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use yuzora_host::herdr_backend::{HerdrMetadata, HerdrRemoteBackend};
use yuzora_host::herdr_service::{HerdrBinarySourceInfo, HerdrManager};

struct Remote {
    calls: Mutex<Vec<(String, Value)>>,
    socket: String,
}
impl HerdrRemoteBackend for Remote {
    fn metadata(&self, query: HerdrMetadata, session: Option<&str>) -> Result<Value, String> {
        if let Some(session) = session {
            assert_eq!(session, "same-name");
        }
        Ok(match query {
            HerdrMetadata::BinarySource => serde_json::to_value(HerdrBinarySourceInfo {
                available: true,
                path: Some("/does-not-exist-on-client/herdr".into()),
                ..Default::default()
            })
            .unwrap(),
            HerdrMetadata::BinaryFingerprint => json!("remote-inode-and-mtime"),
            HerdrMetadata::Sessions => {
                json!({"sessions":[{"name":"same-name","default":true,"running":true,"socket_path":self.socket,"session_dir":"/remote/session"}]})
            }
            HerdrMetadata::Status => {
                json!({"client":{"version":"0.8.2","protocol":20},"server":{"status":"running","running":true,"compatible":true,"version":"0.8.2","protocol":20,"socket":"/wrong/status/socket"}})
            }
            HerdrMetadata::Schema => {
                json!({"protocol":20,"schema_version":1,"methods":["ping","tab.close","session.snapshot","tab.create"]})
            }
        })
    }
    fn request(&self, socket: &str, method: &str, params: Value) -> Result<Value, String> {
        assert_eq!(
            socket, self.socket,
            "only the discovered socket is authoritative"
        );
        self.calls.lock().unwrap().push((method.into(), params));
        Ok(if method == "ping" {
            json!({"result":{"type":"pong","version":"0.8.2","protocol":20}})
        } else {
            json!({"error":{"code":"conflict","message":"changed on host"}})
        })
    }
}

#[test]
fn remote_facade_uses_host_metadata_and_discovered_socket_without_local_binary() {
    for socket in ["/host-a/socket", "/host-b/socket"] {
        let backend = Arc::new(Remote {
            calls: Mutex::default(),
            socket: socket.into(),
        });
        let manager =
            HerdrManager::with_remote("/does-not-exist-on-client/herdr".into(), backend.clone());
        let caps = manager.capabilities_for_session(Some("same-name"));
        assert!(caps.api.tab_close, "{:?}", caps.api.reason);
        assert_eq!(caps.server.socket_path.as_deref(), Some(socket));
        assert_eq!(
            manager
                .tab_close(Some("same-name"), "same-tab".into())
                .unwrap_err(),
            "conflict: changed on host"
        );
        assert_eq!(
            backend
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(method, _)| method == "tab.close")
                .count(),
            1
        );
        assert!(manager.ensure_server_running_on_startup().is_err());
    }
}
