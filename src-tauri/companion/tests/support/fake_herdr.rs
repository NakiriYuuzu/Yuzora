//! Unix test fake: explicit binary + private Unix API socket, never a real Herdr.
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixListener,
    task::{JoinHandle, JoinSet},
};

pub struct FakeHerdr {
    pub root: tempfile::TempDir,
    pub binary: PathBuf,
    pub subscriptions: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}
impl FakeHerdr {
    pub async fn new(mut snapshot: Value) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("yzc-fake-herdr");
        std::fs::write(&binary, include_str!("../fixtures/fake-herdr.sh")).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = root.path().join("api.sock");
        std::fs::write(root.path().join("sessions.json"),json!({"sessions":[{"name":"work","default":true,"running":true,"session_dir":root.path(),"socket_path":socket},{"name":"stopped","default":false,"running":false,"session_dir":root.path(),"socket_path":socket},{"name":"private","default":false,"running":true,"session_dir":root.path(),"socket_path":socket}]}).to_string()).unwrap();
        std::fs::write(root.path().join("status.json"),json!({"client":{"version":"0.9.1","protocol":22},"server":{"status":"running","running":true,"compatible":true,"version":"0.9.1","protocol":22,"socket":socket}}).to_string()).unwrap();
        std::fs::write(root.path().join("schema.json"),json!({"protocol":22,"schema_version":1,"methods":["ping","session.snapshot","events.subscribe","agent.prompt","agent.send_keys"]}).to_string()).unwrap();
        snapshot["version"] = json!("0.9.1");
        snapshot["protocol"] = json!(22);
        let listener = UnixListener::bind(socket).unwrap();
        let subscriptions = Arc::new(AtomicUsize::new(0));
        let active = subscriptions.clone();
        let requests = root.path().join("api-requests");
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    Some(_) = connections.join_next(), if !connections.is_empty()=>{},
                    accepted=listener.accept()=>{
                        let (socket,_)=accepted.unwrap();let snapshot=snapshot.clone();let active=active.clone();let requests=requests.clone();
                        connections.spawn(async move {
                            let mut socket=BufReader::new(socket);let mut line=String::new();
                            if socket.read_line(&mut line).await.unwrap_or(0)==0{return;}
                            let request:Value=serde_json::from_str(&line).unwrap();
                            use std::io::Write;
                            let mut log=std::fs::OpenOptions::new().create(true).append(true).open(requests).unwrap();
                            writeln!(log,"{request}").unwrap();
                            let result=match request["method"].as_str().unwrap() {
                                "ping"=>json!({"type":"pong","version":"0.9.1","protocol":22}),
                                "session.snapshot"=>json!({"type":"session_snapshot","snapshot":snapshot}),
                                "events.subscribe"=>json!({"type":"subscription_started"}),
                                "agent.prompt"|"agent.send_keys"=>json!({"type":"ok"}),
                                other=>panic!("forbidden fake API method: {other}"),
                            };
                            if socket.get_mut().write_all(format!("{}\n",json!({"id":request["id"],"result":result})).as_bytes()).await.is_err() { return; }
                            if request["method"]=="events.subscribe" {
                                active.fetch_add(1,Ordering::SeqCst);
                                let event=json!({"event":"pane.agent_status_changed","data":{"pane_id":"w:p","workspace_id":"w","agent_status":"done","title":"SECRET"}});
                                socket.get_mut().write_all(format!("{event}\n").as_bytes()).await.unwrap();
                                let _=socket.read_u8().await;
                                active.fetch_sub(1,Ordering::SeqCst);
                            }
                        });
                    }
                }
            }
        });
        Self {
            root,
            binary,
            subscriptions,
            task,
        }
    }
    /// From now on a control connector never reads its stdin. (Not every test
    /// binary that includes this module uses it.)
    #[allow(dead_code)]
    pub fn stall_stdin(&self) {
        std::fs::write(self.root.path().join("stall-stdin"), "").unwrap();
    }
    pub fn children(&self) -> Vec<u32> {
        std::fs::read_to_string(self.root.path().join("children"))
            .unwrap_or_default()
            .lines()
            .filter_map(|s| s.parse().ok())
            .collect()
    }
    pub async fn assert_released(&self) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let alive = self.children().into_iter().any(|pid| {
                    let mut command = std::process::Command::new("/bin/kill");
                    for (key, _) in std::env::vars_os() {
                        if key
                            .to_string_lossy()
                            .to_ascii_uppercase()
                            .starts_with("HERDR_")
                        {
                            command.env_remove(key);
                        }
                    }
                    command
                        .args(["-0", &pid.to_string()])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status()
                        .unwrap()
                        .success()
                });
                if !alive && self.subscriptions.load(Ordering::SeqCst) == 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    pub fn assert_environment(&self) {
        let environment = std::fs::read_to_string(self.root.path().join("environment")).unwrap();
        assert!(
            environment.lines().all(|line| line == "HERDR_SESSION=work"),
            "unexpected fake routing env: {environment}"
        );
        let calls = std::fs::read_to_string(self.root.path().join("calls")).unwrap();
        assert!(!calls.lines().any(|line| line == "server"
            || line.starts_with("session start")
            || line.starts_with("session attach")));
    }
}
impl Drop for FakeHerdr {
    fn drop(&mut self) {
        self.task.abort();
    }
}
