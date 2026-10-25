# Yuzora patch: keep workspace assets in the main webview

Upstream: `tauri` 2.12.0, crates.io package SHA-256
`cb20ef8e6f97d6fe03039666cd82861f09a38b56291fa2f0f9fe140023f61c1a`.
The original MIT and Apache-2.0 licenses are retained. Cargo's root
`[patch.crates-io]` keeps the App and plugins on the same patched package.

The built-in `asset` protocol previously let an external Browser child read
files in the App's accumulated workspace scope. Yuzora authorizes those files
for the main editor, not for arbitrary web content.

`src/protocol/asset.rs` now checks the native request's webview ID before
cloning scope, spawning work, or opening a file. Only `main` is allowed;
Browser children and unknown IDs receive an empty 403 response. Origin and
Referer headers do not grant access. This check runs for every request, which
also covers Linux's shared WebContext protocol registration.

The original main-view response implementation remains unchanged, including
scope checks, MIME, encoded paths, HEAD, and single/multiple byte ranges.

`src/test/mod.rs` adds an adapter under the existing `test` feature so Yuzora's
`tests/asset_protocol.rs` exercises the real handler. The adapter is not part
of the production build. Run the regression with:

```sh
cd src-tauri
cargo test --locked --test asset_protocol
```

When updating Tauri, reapply and audit the request-time check, verify the native
ID in each runtime backend (including shared Linux contexts), and rerun both
the handler regression and isolated native Browser/main-image acceptance.
Do not replace this guard with page JavaScript or an Origin-header check.
