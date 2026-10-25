// Yuzora security regressions: exercise the actual asynchronous asset handler.
// Copyright 2026 Yuzora contributors
// SPDX-License-Identifier: Apache-2.0 OR MIT

use std::borrow::Cow;
use tauri::{
    http::{header::*, Request, Response, StatusCode},
    scope,
};
type AssetResponse = Response<Cow<'static, [u8]>>;
type AssetHandler = Box<dyn Fn(&str, Request<Vec<u8>>) -> std::sync::mpsc::Receiver<AssetResponse>>;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

struct Fixture {
    root: PathBuf,
    file: PathBuf,
    outside: PathBuf,
    handler: AssetHandler,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "yuzora-asset-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let allowed = root.join("allowed");
        std::fs::create_dir(&allowed).unwrap();
        let file = allowed.join("document 中文.txt");
        let outside = root.join("outside.txt");
        std::fs::write(&file, b"0123456789abcdef").unwrap();
        std::fs::write(&outside, b"OUTSIDE_SYNTHETIC_MARKER").unwrap();
        let app = tauri::test::mock_app();
        let scope =
            scope::fs::Scope::new(&app, &tauri::utils::config::FsScope::AllowedPaths(vec![]))
                .unwrap();
        scope.allow_directory(&allowed, true).unwrap();
        Self {
            root,
            file,
            outside,
            handler: Box::new(tauri::test::yuzora_asset_protocol_handler(
                scope,
                "tauri://localhost".into(),
            )),
        }
    }

    fn request(
        &self,
        label: &str,
        path: &std::path::Path,
        method: &str,
        range: Option<&str>,
    ) -> Response<Cow<'static, [u8]>> {
        let encoded: String = path
            .to_str()
            .unwrap()
            .bytes()
            .map(|byte| format!("%{byte:02X}"))
            .collect();
        let mut request = Request::builder()
            .method(method)
            .uri(format!("asset://localhost/{encoded}"))
            // A page can supply these headers; neither may grant the main label.
            .header(ORIGIN, "tauri://localhost")
            .header(REFERER, "tauri://localhost/");
        if let Some(range) = range {
            request = request.header(RANGE, range);
        }
        (self.handler)(label, request.body(Vec::new()).unwrap())
            .recv_timeout(Duration::from_secs(5))
            .expect("asset responder did not finish")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn child_and_unknown_webviews_cannot_read_authorized_assets() {
    let fixture = Fixture::new();
    // Use one handler for every caller, as on Linux's shared WebContext.
    assert_eq!(
        fixture
            .request("main", &fixture.file, "GET", None)
            .body()
            .as_ref(),
        b"0123456789abcdef"
    );
    for label in ["preview-child", "", "main-extra"] {
        for (method, range) in [("GET", None), ("HEAD", None), ("GET", Some("bytes=0-3"))] {
            let response = fixture.request(label, &fixture.file, method, range);
            assert_eq!(
                response.status(),
                StatusCode::FORBIDDEN,
                "label={label:?} method={method} range={range:?}"
            );
            assert!(response.body().is_empty());
        }
    }
    assert_eq!(
        fixture
            .request("main", &fixture.file, "GET", None)
            .body()
            .as_ref(),
        b"0123456789abcdef"
    );
}

#[test]
fn main_retains_scope_encoded_paths_head_and_range_responses() {
    let fixture = Fixture::new();
    let full = fixture.request("main", &fixture.file, "GET", None);
    assert_eq!(full.status(), StatusCode::OK);
    assert_eq!(full.headers()[CONTENT_LENGTH], "16");
    assert_eq!(
        full.headers()[ACCESS_CONTROL_ALLOW_ORIGIN],
        "tauri://localhost"
    );
    assert_eq!(full.body().as_ref(), b"0123456789abcdef");
    let head = fixture.request("main", &fixture.file, "HEAD", None);
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()[CONTENT_LENGTH], "16");
    assert!(head.body().is_empty());
    let range = fixture.request("main", &fixture.file, "GET", Some("bytes=2-5"));
    assert_eq!(range.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(range.headers()[CONTENT_RANGE], "bytes 2-5/16");
    assert_eq!(range.body().as_ref(), b"2345");
    let multiple = fixture.request("main", &fixture.file, "GET", Some("bytes=0-1,8-9"));
    assert_eq!(multiple.status(), StatusCode::PARTIAL_CONTENT);
    assert!(multiple.headers()[CONTENT_TYPE]
        .to_str()
        .unwrap()
        .starts_with("multipart/byteranges; boundary="));
    let body = String::from_utf8_lossy(multiple.body());
    assert!(body.contains("content-range: bytes 0-1/16") && body.contains("\r\n\r\n01"));
    assert!(body.contains("content-range: bytes 8-9/16") && body.contains("\r\n\r\n89"));
    assert_eq!(
        fixture
            .request("main", &fixture.file, "GET", Some("bytes=100-200"))
            .status(),
        StatusCode::RANGE_NOT_SATISFIABLE
    );
    assert_eq!(
        fixture
            .request("main", &fixture.outside, "GET", None)
            .status(),
        StatusCode::FORBIDDEN
    );
}
