fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn forbidden_dependency(lock: &str) -> Option<&'static str> {
    let lock = normalize(lock);
    [
        "openssl",
        "openssl-sys",
        "aws-lc-sys",
        "aws-lc-rs",
        "native-tls",
    ]
    .into_iter()
    .find(|package| lock.contains(&format!("name = \"{package}\"\n")))
}

#[test]
fn dependency_features_remain_minimal() {
    let manifest = normalize(include_str!("../Cargo.toml"));
    // SHA-1 is required only by the RFC 6455 upgrade handshake, not authentication.
    assert!(manifest.contains("\"ws\""));
    assert!(manifest.contains("chrono = { version = \"=0.4.45\", default-features = false, features = [\"std\", \"now\"] }"));
    assert!(manifest.contains(
        "rand = { version = \"=0.10.3\", default-features = false, features = [\"sys_rng\"] }"
    ));
    assert_eq!(forbidden_dependency(include_str!("../Cargo.lock")), None);
}

#[test]
fn forbidden_dependency_detection_is_crlf_safe() {
    for newline in ["\n", "\r\n"] {
        for package in [
            "openssl",
            "openssl-sys",
            "aws-lc-sys",
            "aws-lc-rs",
            "native-tls",
        ] {
            let lock = format!(
                "[[package]]{newline}name = \"{package}\"{newline}version = \"1\"{newline}"
            );
            assert_eq!(forbidden_dependency(&lock), Some(package));
        }
        assert_eq!(
            forbidden_dependency(&format!("name = \"safe-package\"{newline}")),
            None
        );
    }
}

fn guarded_router_source(source: &str) -> bool {
    let source: String = normalize(source)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    ![
        ".route(",
        ".nest(",
        ".merge(",
        ".route_service(",
        ".nest_service(",
    ]
    .iter()
    .any(|registration| source.contains(registration))
}

#[test]
fn http_router_has_only_guarded_fallback() {
    let http = normalize(include_str!("../src/http.rs"));
    assert_eq!(http.matches("Router::new()").count(), 1);
    assert_eq!(http.matches(".fallback(handle)").count(), 1);
    let mut directories = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                directories.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let source = normalize(&std::fs::read_to_string(&path).unwrap());
                assert!(
                    guarded_router_source(&source),
                    "unguarded route in {}",
                    path.display()
                );
                if path.file_name().unwrap() != "http.rs" {
                    assert!(
                        !source.contains("Router::new("),
                        "router outside http.rs: {}",
                        path.display()
                    );
                }
            }
        }
    }
}

#[test]
fn router_contract_rejects_each_registration_form() {
    for registration in ["route", "nest", "merge", "route_service", "nest_service"] {
        assert!(!guarded_router_source(&format!(
            "Router::new().{registration}(handler)"
        )));
        assert!(!guarded_router_source(&format!(
            "Router::new()\r\n .{registration} (handler)"
        )));
    }
    assert!(guarded_router_source("Router::new()\r\n .fallback(handle)"));
}

#[test]
fn workflow_tracks_vendor_and_checks_each_forbidden_tls_package() {
    let workflow = normalize(include_str!("../../../.github/workflows/companion.yml"));
    assert!(workflow.contains("  workflow_dispatch:"));
    assert!(workflow.contains("  pull_request:"));
    assert!(!workflow.contains("  push:"));
    assert!(workflow.contains("'src-tauri/vendor/tiberius-0.12.3-yuzora/**'"));
    assert!(workflow.contains("for p in openssl openssl-sys aws-lc-sys aws-lc-rs native-tls; do"));
    assert!(workflow.contains("-i \"$p\""));
    assert!(workflow.contains("grep -F 'did not match any packages'"));
}

#[test]
fn go_sidecar_pins_a_patched_toolchain_and_a_failing_pinned_govulncheck() {
    // setup-go installs the toolchain line of go.mod, else its go line.
    let go_mod = normalize(include_str!("../../companion-net/go.mod"));
    let toolchain = go_mod
        .lines()
        .find_map(|line| line.strip_prefix("toolchain go"))
        .expect("go.mod must name the toolchain the sidecar is built with");
    let version: Vec<u32> = toolchain
        .split('.')
        .map(|part| part.parse().expect("numeric toolchain version"))
        .collect();
    // 1.27.2 and 1.27.3 carry the crypto/tls, net/http and crypto/x509 fixes.
    assert!(version >= vec![1, 27, 3], "toolchain go{toolchain}");

    let workflow = normalize(include_str!("../../../.github/workflows/companion.yml"));
    let net = workflow
        .split("\n  net:\n")
        .nth(1)
        .and_then(|rest| rest.split("\n  supply-chain:\n").next())
        .expect("net job");
    let step = net
        .split("- name: govulncheck\n")
        .nth(1)
        .expect("the net job has a govulncheck step");
    let install = step
        .lines()
        .find(|line| line.contains("go install golang.org/x/vuln/cmd/govulncheck@"))
        .expect("the step installs govulncheck");
    assert!(
        install.contains("govulncheck@v") && !install.contains("@latest"),
        "govulncheck must be pinned: {install}"
    );
    // The shipped binary is built with these tags (scripts/prepare-companion.ts);
    // govulncheck exits 3 for a reachable advisory and nothing may swallow that.
    assert!(step.contains("/govulncheck\" -tags ts_omit_webclient ./..."));
    assert!(step.contains("set -euo pipefail"));
    assert!(!step.contains("|| true") && !step.contains("continue-on-error"));
}

#[test]
fn companion_build_output_is_ignored() {
    assert!(normalize(include_str!("../../../.gitignore"))
        .lines()
        .any(|line| line == "src-tauri/companion/target/"));
}

#[test]
fn proxy_secret_is_never_debug_or_serialize_and_guard_ignores_the_old_header() {
    let admin = normalize(include_str!("../src/admin.rs"));
    let before = admin.split("pub struct ProxySecret").next().unwrap();
    assert_eq!(
        before
            .lines()
            .rev()
            .find(|line| line.starts_with("#[derive")),
        Some("#[derive(Clone)]")
    );
    for forbidden in ["Debug for ProxySecret", "Serialize for ProxySecret"] {
        assert!(!admin.contains(forbidden));
    }
    // Production code only; the guard tests name the old header on purpose.
    let guard = normalize(include_str!("../src/guard.rs"));
    let production = guard.split("#[cfg(test)]").next().unwrap();
    assert!(!production
        .to_ascii_lowercase()
        .contains("tailscale-user-login"));
    assert!(
        production.contains("\"yzc-proxy-secret\"") && production.contains("\"yzc-peer-login\"")
    );
}

#[test]
fn git_bridge_only_detects_and_reads_and_never_touches_trust_grants() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let code = |source: &str| {
        normalize(source)
            .split("#[cfg(test)]")
            .next()
            .unwrap()
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let bridge = code(&std::fs::read_to_string(root.join("git_bridge.rs")).unwrap());
    // Git is reached through typed read functions; the one command named is Detect.
    let commands: Vec<_> = bridge
        .match_indices("GitCommand::")
        .map(|(at, _)| {
            bridge[at..]
                .split(|c: char| !(c.is_alphanumeric() || c == ':'))
                .next()
                .unwrap()
        })
        .collect();
    assert_eq!(commands, ["GitCommand::Detect"]);
    for forbidden in [
        "grant",
        "revoke",
        "challenge",
        "mutate",
        "FileAtRev",
        "RemoteProbe",
        "Bootstrap",
        ".execute(",
        "files::",
    ] {
        assert!(
            !bridge.contains(forbidden),
            "git_bridge.rs names {forbidden}"
        );
    }
    // No other module reaches the trust store.
    for entry in std::fs::read_dir(&root).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_owned();
        if path.extension().is_some_and(|ext| ext == "rs") && name != "git_bridge.rs" {
            let source = code(&std::fs::read_to_string(&path).unwrap());
            assert!(
                !source.contains("workspace_trust"),
                "{name} reaches the workspace trust store"
            );
        }
    }
}

#[test]
fn push_module_never_touches_the_network_or_key_material() {
    // The desktop holds the VAPID key and sends every push; the gateway only
    // decides that one is due.
    let push = normalize(include_str!("../src/push.rs"));
    let code = push
        .split("#[cfg(test)]")
        .next()
        .unwrap()
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in [
        "TcpStream",
        "TcpListener",
        "UdpSocket",
        "reqwest",
        "hyper::client",
        "hyper_util::client",
        "p256::",
        "ecdsa::",
        "aes_gcm::",
        "hkdf::",
        "SigningKey",
        "private",
    ] {
        assert!(!code.contains(forbidden), "push.rs names {forbidden}");
    }
    let manifest = normalize(include_str!("../Cargo.toml"));
    let dependencies = manifest.split("[dev-dependencies]").next().unwrap();
    for crate_name in ["reqwest", "p256", "hkdf", "aes-gcm"] {
        assert!(
            !dependencies.contains(&format!("\n{crate_name} =")),
            "{crate_name} is a production dependency"
        );
    }
}

#[test]
fn webauthn_pinned_and_no_public_raw_session_id_authorization_api() {
    let manifest = normalize(include_str!("../Cargo.toml"));
    assert!(
        manifest.contains("webauthn_rp = { version = \"=0.3.0\", features = [\"serde_relaxed\"] }")
    );
    let session = normalize(include_str!("../src/session.rs"));
    for signature in session.split("pub fn ").skip(1) {
        let signature = signature.split('{').next().unwrap();
        assert!(
            !signature.split(")").next().unwrap().contains("&[u8; 32]"),
            "raw session-id public API: {signature}"
        );
    }
    assert!(session.contains("    fn elevate(&mut self, ctx: &AuthContext"));
    assert!(normalize(include_str!("../src/gateway.rs")).contains("never async or return a future"));
}

#[test]
fn supply_chain_exception_is_version_pinned_and_not_expired() {
    let deadline = chrono::NaiveDate::from_ymd_opt(2027, 3, 31).unwrap();
    assert!(
        chrono::Utc::now().date_naive() <= deadline,
        "RSA advisory exception expired: Yuzora maintainers must re-review"
    );
    let lock = normalize(include_str!("../Cargo.lock"));
    assert_eq!(
        lock.lines()
            .filter(|line| *line == "name = \"rsa\"")
            .count(),
        1
    );
    assert!(lock.contains("name = \"rsa\"\nversion = \"0.9.10\""));
    for config in [
        include_str!("../deny.toml"),
        include_str!("../.cargo/audit.toml"),
    ] {
        for marker in [
            "RUSTSEC-2023-0071",
            "rsa 0.9.10",
            "Yuzora maintainers",
            "2027-03-31",
            "ES256",
        ] {
            assert!(config.contains(marker));
        }
    }
    for line in lock.lines() {
        assert!(
            !line.starts_with("name = \"aws-lc"),
            "forbidden AWS-LC package"
        );
    }
    let workflow = normalize(include_str!("../../../.github/workflows/companion.yml"));
    assert!(workflow.contains("cargo-deny --version 0.20.2"));
    assert!(workflow.contains("cargo-audit --version 0.22.2"));
}

#[test]
fn p256_bigint_exception_is_pinned_and_production_never_holds_private_keys() {
    let deadline = chrono::NaiveDate::from_ymd_opt(2027, 3, 31).unwrap();
    assert!(
        chrono::Utc::now().date_naive() <= deadline,
        "crypto-bigint advisory exception expired: Yuzora maintainers must re-review"
    );
    let lock = normalize(include_str!("../Cargo.lock"));
    assert_eq!(
        lock.lines()
            .filter(|line| *line == "name = \"crypto-bigint\"")
            .count(),
        1
    );
    assert!(lock.contains("name = \"crypto-bigint\"\nversion = \"0.5.5\""));
    assert!(lock.contains("name = \"p256\"\nversion = \"0.13.2\""));
    for config in [
        include_str!("../deny.toml"),
        include_str!("../.cargo/audit.toml"),
    ] {
        // The quoted id is the active ignore entry; comments never quote it.
        assert!(config.contains("\"RUSTSEC-2026-0387\""));
        for marker in [
            "crypto-bigint 0.5.5",
            "Yuzora maintainers",
            "2027-03-31",
            "ES256",
        ] {
            assert!(config.contains(marker), "missing {marker}");
        }
    }
    // The exception holds only while production never touches private keys:
    // none of these crates is a production dependency (in any manifest table
    // form, under any rename) and no production source names secret-key types.
    // yuzora-host is compiled into the gateway, so it is covered too.
    let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let companion_manifest = read_manifest(&manifest_dir.join("Cargo.toml"));
    let host_manifest = read_manifest(&manifest_dir.join("../host/Cargo.toml"));
    for manifest in [&companion_manifest, &host_manifest] {
        for (key, value) in production_dependencies(manifest) {
            let package = value
                .get("package")
                .and_then(toml::Value::as_str)
                .unwrap_or(&key);
            for crate_name in P256_FAMILY {
                assert!(
                    key != *crate_name && package != *crate_name,
                    "{crate_name} is a production dependency ({key})"
                );
            }
        }
    }
    assert!(companion_manifest["dev-dependencies"]
        .as_table()
        .unwrap()
        .contains_key("p256"));
    let mut inspected = 0;
    for root in [manifest_dir.join("src"), manifest_dir.join("../host/src")] {
        let mut directories = vec![root];
        while let Some(directory) = directories.pop() {
            for entry in std::fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                let name = path.file_name().unwrap().to_str().unwrap().to_owned();
                if path.is_dir() {
                    directories.push(path);
                } else if name.ends_with(".rs")
                    && name != "tests.rs"
                    && !name.ends_with("_tests.rs")
                    && name != "test_support.rs"
                {
                    let source = production_source(&std::fs::read_to_string(&path).unwrap());
                    let code = source
                        .lines()
                        .filter(|line| !line.trim_start().starts_with("//"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    for forbidden in [
                        "SigningKey",
                        "SecretKey",
                        "NonZeroScalar",
                        "Scalar",
                        "ecdh",
                        "diffie_hellman",
                        "crypto_bigint",
                        "elliptic_curve",
                    ] {
                        assert!(
                            !code.contains(forbidden),
                            "{} names {forbidden}",
                            path.display()
                        );
                    }
                    inspected += 1;
                }
            }
        }
    }
    assert!(
        inspected >= 40,
        "production source scan found {inspected} files"
    );
}

const P256_FAMILY: &[&str] = &["p256", "p384", "ecdsa", "elliptic-curve", "crypto-bigint"];

fn read_manifest(path: &std::path::Path) -> toml::Table {
    std::fs::read_to_string(path).unwrap().parse().unwrap()
}

// Every non-dev dependency entry: [dependencies], [build-dependencies] and the
// same two under [target.<cfg>]. Entries are normalized to tables so inline,
// dotted ([dependencies.name]) and renamed (`package = ...`) forms look alike.
fn production_dependencies(manifest: &toml::Table) -> Vec<(String, toml::Value)> {
    let mut tables = Vec::new();
    for kind in ["dependencies", "build-dependencies"] {
        tables.push(manifest.get(kind));
    }
    if let Some(targets) = manifest.get("target").and_then(toml::Value::as_table) {
        for target in targets.values() {
            for kind in ["dependencies", "build-dependencies"] {
                tables.push(target.get(kind));
            }
        }
    }
    let mut entries = Vec::new();
    for table in tables.into_iter().flatten() {
        for (key, value) in table.as_table().unwrap() {
            let value = if value.is_table() {
                value.clone()
            } else {
                toml::Value::Table(toml::Table::new())
            };
            entries.push((key.clone(), value));
        }
    }
    entries
}

#[test]
fn auth_context_cannot_supply_cached_scope_authorization() {
    let session = normalize(include_str!("../src/session.rs"));
    let context = session
        .split("pub struct AuthContext {")
        .nth(1)
        .unwrap()
        .split('}')
        .next()
        .unwrap();
    assert!(!context.contains("scope:"));
    let accessors = session
        .split("impl AuthContext {")
        .nth(1)
        .unwrap()
        .split("struct Peer")
        .next()
        .unwrap();
    assert!(!accessors.contains("fn scope("));
    for entry in
        std::fs::read_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")).unwrap()
    {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "rs") {
            let source = normalize(&std::fs::read_to_string(path).unwrap());
            let compact: String = source.chars().filter(|c| !c.is_whitespace()).collect();
            assert!(
                !compact.contains(".scope()"),
                "authorization must use each method's declared scope"
            );
        }
    }
}

#[test]
fn webauthn_o2_verifier_layers_and_test_only_filter_seam() {
    let source = normalize(include_str!("../src/webauthn.rs"));
    assert!(source.contains("backup_requirement: if requires_unbacked(policy, scope)"));
    assert!(source.contains("|| !policy_accepts(policy, scope, be, bs)"));
    assert!(source.contains("#[cfg(test)]\n#[path = \"webauthn_tests.rs\"]\nmod tests;"));
    let seam = "fn unfiltered_elevation_for_test(";
    assert!(normalize(include_str!("../src/webauthn_tests.rs")).contains(seam));
    // Its sole definition/call sites are in the cfg(test)-only module, never in
    // the library/HTTP router compiled for the production integration binary.
    for name in [
        "webauthn.rs",
        "gateway_auth.rs",
        "http.rs",
        "gateway.rs",
        "lib.rs",
    ] {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .join(name),
        )
        .unwrap();
        assert!(!source.contains("unfiltered_elevation_for_test"));
    }
}

// Ignore only rustfmt's exact top-level cfg(test) inline module shape. Stop at
// its top-level closing brace, NOT the end of the file (production can follow).
// Unrecognized shapes remain scanned, i.e. fail closed rather than hide code.
fn production_source(source: &str) -> String {
    let mut source = normalize(source);
    let marker = "#[cfg(test)]\nmod tests {\n";
    while let Some(start) = source.find(marker) {
        let end = start + marker.len();
        let close = if source[end..].starts_with("}\n") {
            2 // Empty test module must not swallow following production code.
        } else if let Some(close) = source[end..].find("\n}\n") {
            close + 3
        } else {
            break;
        };
        source.replace_range(start..end + close, "");
    }
    source
}
fn owned_tasks_only(name: &str, source: &str) -> bool {
    let source = production_source(source);
    let mut compact: String = source.chars().filter(|c| !c.is_whitespace()).collect();
    if ["WebSocketUpgrade", "on_upgrade(", "extract::ws"]
        .iter()
        .any(|s| compact.contains(s))
    {
        return false;
    }
    if name == "admin.rs" {
        // The sole exception is inside the existing stdout writer, not its file.
        let Some(start) = compact.find("pubfnspawn_writer<") else {
            return false;
        };
        let Some(end) = compact[start..].find("asyncfnsend<") else {
            return false;
        };
        let writer = &compact[start..start + end];
        if writer.matches("tokio::spawn(").count() != 1 {
            return false;
        }
        let allowed = writer.find("tokio::spawn(").unwrap() + start;
        compact.replace_range(allowed..allowed + "tokio::spawn(".len(), "");
    }
    // Text tripwire only, not Rust name/type resolution: review must prove that
    // permitted method calls really belong to gateway/peer-owned JoinSets.
    if [
        "Handle::current",
        "Handle::try_current",
        "axum::serve",
        "usetokio::spawn",
        "usetokio::task::spawn",
        "thread::Builder",
        "task::Builder",
        "usestd::thread::spawn",
        "spawnas",
    ]
    .iter()
    .any(|s| compact.contains(s))
    {
        return false;
    }
    if name != "main.rs" && (compact.contains("runtime::Builder") || compact.contains("Runtime::"))
    {
        return false;
    }
    for group in ["usetokio::task::{", "usestd::thread::{", "usetokio::{"] {
        if compact
            .split(group)
            .skip(1)
            .any(|s| s.split('}').next().unwrap_or("").contains("spawn"))
        {
            return false;
        }
    }
    for call in ["spawn(", "spawn_blocking(", "spawn_local("] {
        for (index, _) in compact.match_indices(call) {
            if index == 0 || compact.as_bytes()[index - 1] != b'.' {
                return false;
            }
        }
    }
    true
}

fn test_file_only_once(sources: &[String], name: &str) -> bool {
    let stem = name.strip_suffix(".rs").unwrap();
    let marker = format!("#[cfg(test)]#[path=\"{name}\"]modtests;");
    sources
        .iter()
        .map(|s| {
            s.split('"')
                .enumerate()
                .filter(|(i, literal)| i % 2 == 1 && literal.rsplit('/').next() == Some(name))
                .count()
        })
        .sum::<usize>()
        == 1
        && sources.iter().filter(|s| s.contains(&marker)).count() == 1
        && !sources.iter().any(|s| s.contains(&format!("mod{stem};")))
}

#[test]
fn websocket_upgrade_and_task_ownership_production_contract() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let lib = normalize(&std::fs::read_to_string(root.join("lib.rs")).unwrap());
    assert!(lib.contains("#[cfg(test)]\nmod test_support;"));
    let mut sources = Vec::new();
    let mut inventory = vec![root.clone()];
    while let Some(directory) = inventory.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                inventory.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                sources.push(
                    normalize(&std::fs::read_to_string(path).unwrap())
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .collect::<String>(),
                );
            }
        }
    }
    let mut directories = vec![root.clone()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                directories.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let name = path.file_name().unwrap().to_str().unwrap();
            if name == "test_support.rs" {
                continue;
            }
            if let Some(base) = name.strip_suffix("_tests.rs") {
                // Prove each excluded file is a cfg(test)-only companion module.
                let parent =
                    normalize(&std::fs::read_to_string(root.join(format!("{base}.rs"))).unwrap());
                assert!(parent.contains(&format!("#[cfg(test)]\n#[path = \"{name}\"]\nmod tests;")));
                assert!(
                    test_file_only_once(&sources, name),
                    "test file multiply or unconditionally included: {name}"
                );
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            assert!(
                owned_tasks_only(name, &source),
                "unowned task/upgrade in {}",
                path.display()
            );
        }
    }
    assert!(normalize(include_str!("../Cargo.toml")).contains("\"ws\""));
    assert!(normalize(include_str!("../src/http.rs")).contains("Access::FirstFrame(_)"));
}

#[test]
fn websocket_ownership_contract_catches_production_after_test_module_and_admin_outside_writer() {
    let tests = "#[cfg(test)]\nmod tests {\n    fn test() { tokio::spawn(async {}); }\n}\n";
    for forbidden in [
        "WebSocketUpgrade",
        "on_upgrade(",
        "extract::ws",
        "tokio::spawn(",
        "tokio :: spawn (",
        "tokio::task::spawn_blocking(",
        "tokio::task::spawn(",
        "task::spawn(",
        "spawn(",
        "spawn_blocking(",
        "spawn_local(",
        "std::thread::spawn(",
        "thread::spawn(",
        "Handle::current()",
        "Handle::try_current()",
        "axum::serve(",
        "use tokio::spawn;",
        "use tokio::task::spawn;",
        "use tokio::task::{JoinSet,spawn_blocking};",
        "std::thread::Builder::new()",
        "tokio::task::Builder::new()",
        "tokio::runtime::Builder::new_current_thread()",
        "Runtime::new()",
        "use std::thread::spawn;",
        "use std::thread::{self, spawn};",
        "use tokio::{spawn};",
        "use tokio::task::spawn as hidden;",
    ] {
        for source in [
            format!("fn production() {{ {forbidden} }}\n{tests}"),
            format!("{tests}fn production() {{ {forbidden} }}"),
        ] {
            assert!(!owned_tasks_only("bridge.rs", &source));
        }
    }
    assert!(owned_tasks_only("main.rs", tests));
    assert!(owned_tasks_only(
        "main.rs",
        "tokio::runtime::Builder::new_current_thread();Runtime::new();"
    ));
    assert!(owned_tasks_only(
        "main.rs",
        "tasks.spawn(async {}); tasks.spawn_blocking(|| {});"
    ));
    assert!(!owned_tasks_only(
        "main.rs",
        "#[cfg(test)]\nmod tests {\n}\nfn production() { std::thread::spawn(|| {}); }"
    ));
    let test_only = "#[cfg(test)]#[path=\"bridge_tests.rs\"]modtests;".to_owned();
    assert!(test_file_only_once(
        std::slice::from_ref(&test_only),
        "bridge_tests.rs"
    ));
    for extra in [
        "#[path=\"bridge_tests.rs\"]modleaked;",
        "#[cfg(test)]#[path=\"bridge_tests.rs\"]modtests;",
        "modbridge_tests;",
        "#[path=\"./bridge_tests.rs\"]modleaked;",
    ] {
        assert!(!test_file_only_once(
            &[test_only.clone(), extra.into()],
            "bridge_tests.rs"
        ));
    }
    assert!(!owned_tasks_only(
        "main.rs",
        "mod tests { fn f() { tokio::spawn(async {}); } }"
    ));
    let admin = include_str!("../src/admin.rs");
    assert!(owned_tasks_only("admin.rs", admin));
    assert!(!owned_tasks_only(
        "admin.rs",
        &format!("{admin}\nfn other() {{ tokio::spawn(async {{}}); }}")
    ));
}
