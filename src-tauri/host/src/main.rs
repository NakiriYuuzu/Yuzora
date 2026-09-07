#[cfg(unix)]
fn main() {
    if std::env::var_os(yuzora_host::db_query_worker::WORKER_ENV).is_some() {
        std::process::exit(yuzora_host::db_query_worker::run());
    }
    if matches!(
        std::env::args().nth(1).as_deref(),
        Some("--stdio" | "--stream")
    ) {
        if let Err(error) = yuzora_host::login_env::initialize() {
            eprintln!("host account environment unavailable: {error}; using inherited environment");
        }
    }
    if std::env::args().nth(1).as_deref() == Some("--database") {
        if let Err(error) = yuzora_host::db_query_worker::apply_process_memory_limit(
            yuzora_host::db_query_worker::HELPER_MEMORY_BYTES,
        ) {
            eprintln!("SQLite memory limit setup failed: {error}");
            std::process::exit(2);
        }
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("host runtime")
        .block_on(run());
}

#[cfg(unix)]
async fn run() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let input = yuzora_host::stdio::StdioPipe::input().expect("helper stdin must be a pipe");
    let output = yuzora_host::stdio::StdioPipe::output().expect("helper stdout must be a pipe");
    let result = if args == ["--stdio"] {
        yuzora_host::server::serve(input, output).await
    } else if args == ["--stream"] {
        yuzora_host::streams::serve(input, output).await
    } else if args == ["--tcp"] {
        yuzora_host::tunnel::serve(input, output).await
    } else if args == ["--database"] {
        yuzora_host::sqlite_lane::serve(input, output).await
    } else {
        eprintln!("usage: yuzora-host --stdio | --stream | --tcp | --database");
        std::process::exit(2);
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(not(unix))]
fn main() {
    eprintln!("yuzora-host requires Linux or macOS; use WSL on Windows");
    std::process::exit(2);
}
