//! CPU attribution for the owned SSH fixture; never connects to user hosts.
use super::*;

#[derive(Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CpuPoint {
    process_ns: u64,
    server_thread_ns: u64,
    wall_ns: u64,
    server_tasks: usize,
}

fn thread_cpu_ns() -> u64 {
    let mut time: libc::timespec = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) },
        0
    );
    time.tv_sec as u64 * 1_000_000_000 + time.tv_nsec as u64
}

pub(super) struct ServerThread {
    requests: mpsc::Sender<oneshot::Sender<CpuPoint>>,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for ServerThread {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

impl ServerThread {
    pub(super) async fn start(probe: Arc<Probe>, password: String) -> (SpawnedServers, Self) {
        let (ready, servers) = oneshot::channel();
        let (requests, mut requested) = mpsc::channel::<oneshot::Sender<CpuPoint>>(4);
        let (shutdown, mut stopped) = oneshot::channel();
        let thread = std::thread::Builder::new()
            .name("owned-ssh-server".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                let started = std::time::Instant::now();
                runtime.block_on(async move {
                    // Listeners and their reactor registrations belong to this
                    // runtime, along with all SSH server/backend child tasks.
                    let running = spawn_owned_servers(probe, password).await;
                    if ready.send(running).is_err() {
                        return;
                    }
                    loop {
                        tokio::select! {
                            _ = &mut stopped => break,
                            request = requested.recv() => {
                                let Some(reply) = request else { break };
                                let point = CpuPoint {
                                    server_thread_ns: thread_cpu_ns(),
                                    process_ns: process_cpu_ns(),
                                    wall_ns: started.elapsed().as_nanos() as u64,
                                    server_tasks: tokio::runtime::Handle::current()
                                        .metrics().num_alive_tasks(),
                                };
                                let _ = reply.send(point);
                            }
                        }
                    }
                });
            })
            .unwrap();
        let owned = Self {
            requests,
            shutdown: Some(shutdown),
            thread: Some(thread),
        };
        (servers.await.unwrap(), owned)
    }

    async fn snapshot(&self) -> CpuPoint {
        let (reply, received) = oneshot::channel();
        self.requests.send(reply).await.unwrap();
        received.await.unwrap()
    }

    async fn stop(mut self) {
        self.shutdown.take().unwrap().send(()).unwrap();
        let thread = self.thread.take().unwrap();
        tokio::task::spawn_blocking(move || thread.join().unwrap())
            .await
            .unwrap();
    }
}

async fn attributed_cycle(fixture: &mut Fixture, server: &ServerThread) -> [CpuPoint; 5] {
    let start = server.snapshot().await;
    let deferred = fixture.begin_deferred_open().await;
    let opened = server.snapshot().await;
    let pending = fixture.cancel_owner(deferred).await;
    let cancelled = server.snapshot().await;
    fixture.confirm_pending(pending).await;
    let confirmed = server.snapshot().await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    let settled = server.snapshot().await;
    [start, opened, cancelled, confirmed, settled]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "manual owned SSH client/server CPU attribution; run serially"]
async fn performance_owned_tcp_cancel_cpu_attribution() {
    tokio::time::timeout(Duration::from_secs(120), async {
        const WARMUP: usize = 100;
        const CYCLES: usize = 200;
        let mut calibration = vec![[CpuPoint::default(); 2]; 20];
        let mut points = vec![[CpuPoint::default(); 5]; CYCLES];
        let mut samples = vec![ResourceSample::default(); 11];
        let (mut fixture, server) = Fixture::new_with_server_thread(true).await;
        let server = server.unwrap();
        let mut memory = sysinfo::System::new();
        for pair in &mut calibration {
            *pair = [server.snapshot().await, server.snapshot().await];
        }
        for _ in 0..WARMUP {
            attributed_cycle(&mut fixture, &server).await;
        }
        let initial = server.snapshot().await;
        samples[0] = fixture.sample(0, &mut memory);
        samples[0].alive_tasks += initial.server_tasks;
        for (index, result) in points.iter_mut().enumerate() {
            *result = attributed_cycle(&mut fixture, &server).await;
            let cycle = index + 1;
            if cycle % 20 == 0 {
                let mut sample = fixture.sample(cycle, &mut memory);
                sample.alive_tasks += result[4].server_tasks;
                samples[cycle / 20] = sample;
            }
        }
        println!(
            "SSH_CPU_ATTRIBUTION {}",
            serde_json::json!({
                "warmup": WARMUP, "cycles": CYCLES,
                "phases": ["open", "ownerCancel", "lateConfirmation", "settle"],
                "calibrationPoints": calibration, "points": points, "samples": samples,
                "serverClock": "CLOCK_THREAD_CPUTIME_ID",
                "scope": "Owned server/backend on one dedicated runtime thread; difference from process CPU includes client and test driver. Memory/FDs still combined; snapshot overhead retained, not subtracted."
            })
        );
        fixture.stop().await;
        server.stop().await;
        println!("SSH_CPU_ATTRIBUTION_END {{\"ownedServersStopped\":true}}");
    })
    .await
    .expect("owned SSH CPU attribution deadline");
}
