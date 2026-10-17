//! Fixtures are source-derived from tailscale v1.104.1 (`ipnstate.Status`,
//! `ipn.ServeConfig`), not captured output. Spike A replaces them with real
//! `status --json` / `serve status --json` captures.
use super::*;
use std::collections::HashSet;

const HOST: &str = "desktop.tail1234.ts.net";
const GATEWAY: u16 = 43123;

fn status(name: &str) -> Node {
    let json = match name {
        "running" => include_str!("fixtures/status-running.json"),
        "https-off" => include_str!("fixtures/status-https-off.json"),
        "tagged" => include_str!("fixtures/status-tagged.json"),
        "needs-login" => include_str!("fixtures/status-needs-login.json"),
        _ => unreachable!(),
    };
    parse_status(json).unwrap()
}
fn serve(name: &str) -> Serve {
    let json = match name {
        "null" => "null\n",
        "empty" => "{}\n",
        "ours" => include_str!("fixtures/serve-ours.json"),
        "ours-and-other-port" => include_str!("fixtures/serve-ours-and-other-port.json"),
        "taken" => include_str!("fixtures/serve-taken.json"),
        "extra-mount" => include_str!("fixtures/serve-extra-mount.json"),
        "tcp-forward" => include_str!("fixtures/serve-tcp-forward.json"),
        "funnel" => include_str!("fixtures/serve-funnel.json"),
        "foreground" => include_str!("fixtures/serve-foreground.json"),
        "service" => include_str!("fixtures/serve-service.json"),
        _ => unreachable!(),
    };
    check_serve(json, HOST, 8443, GATEWAY).unwrap()
}

#[test]
fn companion_tailscale_status_running_node() {
    assert_eq!(
        status("running"),
        Node {
            version: "1.104.1-t0000000000-g0000000000".into(),
            backend_state: "Running".into(),
            dns_name: Some(HOST.into()),
            login: Some("user@example.com".into()),
            tagged: false,
            magic_dns: true,
            https_ready: true,
        }
    );
}

#[test]
fn companion_tailscale_status_https_tagged_and_logged_out() {
    let off = status("https-off");
    assert!(!off.https_ready);
    assert_eq!(off.login.as_deref(), Some("user@example.com"));
    let tagged = status("tagged");
    assert!(tagged.tagged);
    let logged_out = status("needs-login");
    assert_eq!(logged_out.backend_state, "NeedsLogin");
    assert_eq!(logged_out.dns_name, None);
    assert_eq!(logged_out.login, None);
    assert!(!logged_out.magic_dns && !logged_out.https_ready);
    assert!(parse_status("not json")
        .unwrap_err()
        .starts_with("tailscale-invalid-status"));
}

#[test]
fn companion_tailscale_https_needs_capability_and_certificate() {
    // The capability alone is what the CLI checks; a certificate domain for
    // this exact name is what the browser needs.
    let mut json: Value =
        serde_json::from_str(include_str!("fixtures/status-running.json")).unwrap();
    json["CertDomains"] = json!(["other.tail1234.ts.net"]);
    assert!(!parse_status(&json.to_string()).unwrap().https_ready);
    let mut json: Value =
        serde_json::from_str(include_str!("fixtures/status-running.json")).unwrap();
    json["Self"].as_object_mut().unwrap().remove("CapMap");
    assert!(!parse_status(&json.to_string()).unwrap().https_ready);
}

#[test]
fn companion_tailscale_serve_port_states() {
    for (name, port) in [
        ("null", PortState::Free),
        ("empty", PortState::Free),
        ("ours", PortState::Ours),
        ("ours-and-other-port", PortState::Ours),
        ("taken", PortState::Taken),
        ("extra-mount", PortState::Shared),
        ("tcp-forward", PortState::Taken),
        ("funnel", PortState::Free),
        ("foreground", PortState::Taken),
        ("service", PortState::Free),
    ] {
        assert_eq!(serve(name).port, port, "{name}");
    }
    // Our handler for a previous gateway port is not ours to replace.
    let stale = check_serve(include_str!("fixtures/serve-ours.json"), HOST, 8443, 40000).unwrap();
    assert_eq!(stale.port, PortState::Taken);
    assert!(check_serve(r#"{"TCP":"8443"}"#, HOST, 8443, GATEWAY)
        .unwrap_err()
        .starts_with("tailscale-invalid-serve"));
}

#[test]
fn companion_tailscale_serve_funnel_and_cookie_sharing_ports() {
    assert!(serve("funnel").funnel);
    assert!(!serve("ours").funnel);
    assert_eq!(serve("ours-and-other-port").other_ports, vec![443]);
    assert_eq!(serve("funnel").other_ports, vec![443]);
    assert!(serve("ours").other_ports.is_empty());
    // Services use their own hostnames, so they share neither ports nor cookies.
    assert!(serve("service").other_ports.is_empty());
    let mut fg: Value =
        serde_json::from_str(include_str!("fixtures/serve-foreground.json")).unwrap();
    fg["Foreground"]["5f1b2c3d4e"]["AllowFunnel"] = json!({format!("{HOST}:8443"): true});
    assert!(
        check_serve(&fg.to_string(), HOST, 8443, GATEWAY)
            .unwrap()
            .funnel
    );
}

#[test]
fn companion_tailscale_refusals_in_order() {
    let ready = status("running");
    assert_eq!(refusal(&ready, &serve("null")), None);
    assert_eq!(refusal(&ready, &serve("ours")), None);
    assert_eq!(refusal(&ready, &serve("ours-and-other-port")), None);
    assert_eq!(
        refusal(&status("needs-login"), &serve("null")),
        Some(Refusal::NotRunning)
    );
    assert_eq!(
        refusal(&status("tagged"), &serve("null")),
        Some(Refusal::Tagged)
    );
    assert_eq!(
        refusal(&status("https-off"), &serve("null")),
        Some(Refusal::HttpsNotReady)
    );
    assert_eq!(refusal(&ready, &serve("funnel")), Some(Refusal::Funnel));
    assert_eq!(refusal(&ready, &serve("taken")), Some(Refusal::PortTaken));
    assert_eq!(
        refusal(&ready, &serve("extra-mount")),
        Some(Refusal::PortTaken)
    );
    assert_eq!(
        refusal(&ready, &serve("foreground")),
        Some(Refusal::PortTaken)
    );
    let mut anonymous = ready.clone();
    anonymous.login = None;
    assert_eq!(refusal(&anonymous, &serve("null")), Some(Refusal::NoLogin));
}

#[test]
fn companion_tailscale_commands_add_and_remove_only_ours() {
    assert_eq!(
        serve_args(8443, GATEWAY),
        ["serve", "--bg", "--https=8443", "http://127.0.0.1:43123"]
    );
    // Only our root mount goes; a mount the user added keeps the port.
    for name in ["ours", "extra-mount"] {
        assert_eq!(
            off_args(&serve(name), 8443).unwrap(),
            ["serve", "--https=8443", "--set-path=/", "off"],
            "{name}"
        );
    }
    for name in ["null", "taken", "tcp-forward", "foreground"] {
        assert_eq!(off_args(&serve(name), 8443), None, "{name}");
    }
}

#[test]
fn companion_tailscale_discover_variants() {
    let found = |paths: &[&str]| {
        let set: HashSet<PathBuf> = paths.iter().map(PathBuf::from).collect();
        move |p: &Path| set.contains(p)
    };
    let dirs = [
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ];
    let bundled = "/Applications/Tailscale.app/Contents/MacOS/Tailscale";
    let receipt = "/Applications/Tailscale.app/Contents/_MASReceipt";
    assert_eq!(
        discover_in("macos", &dirs, None, found(&[bundled, receipt])),
        Some(Cli {
            path: bundled.into(),
            variant: Variant::MacAppStore
        })
    );
    assert_eq!(
        discover_in(
            "macos",
            &dirs,
            None,
            found(&[bundled, "/usr/local/bin/tailscale"])
        ),
        Some(Cli {
            path: "/usr/local/bin/tailscale".into(),
            variant: Variant::MacStandalone
        })
    );
    assert_eq!(
        discover_in(
            "macos",
            &dirs,
            None,
            found(&["/opt/homebrew/bin/tailscale"])
        ),
        Some(Cli {
            path: "/opt/homebrew/bin/tailscale".into(),
            variant: Variant::Cli
        })
    );
    assert_eq!(discover_in("macos", &dirs, None, found(&[])), None);
    let program_files = Path::new("C:/Program Files");
    let windows = program_files.join("Tailscale").join("tailscale.exe");
    assert_eq!(
        discover_in("windows", &[], Some(program_files), |p| p == windows),
        Some(Cli {
            path: windows.clone(),
            variant: Variant::Cli
        })
    );
    assert_eq!(
        discover_in("linux", &dirs, None, found(&["/usr/local/bin/tailscale"])),
        Some(Cli {
            path: "/usr/local/bin/tailscale".into(),
            variant: Variant::Cli
        })
    );
    assert_eq!(discover_in("linux", &dirs, None, found(&[bundled])), None);
}

#[cfg(unix)]
#[tokio::test]
async fn companion_tailscale_run_output_failure_and_timeout() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("yuzora-tailscale-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = |name: &str, body: &str| {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    };
    let ok = script("ok", "printf '%s' \"$*\"");
    let fail = script("fail", "echo 'serve config denied' >&2; exit 1");
    let slow = script("slow", "sleep 5");
    let args = serve_args(8443, GATEWAY);
    assert_eq!(
        run(&ok, &args).await.unwrap(),
        "serve --bg --https=8443 http://127.0.0.1:43123"
    );
    assert_eq!(
        run(&fail, &args).await.unwrap_err(),
        "tailscale-failed: serve config denied"
    );
    assert_eq!(
        run_within(&slow, &args, Duration::from_millis(200))
            .await
            .unwrap_err(),
        "tailscale-timeout"
    );
    assert!(run(&dir.join("missing"), &args)
        .await
        .unwrap_err()
        .starts_with("tailscale-unavailable"));
    std::fs::remove_dir_all(&dir).unwrap();
}
