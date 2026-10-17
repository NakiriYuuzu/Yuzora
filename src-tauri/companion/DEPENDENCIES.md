# Companion Phase 2B dependencies / supply-chain gate

## Selected verifier and persistence

- Exact `webauthn_rp =0.3.0`, default bin/serde plus `serde_relaxed`; all browser registration/assertion JSON uses Relaxed parsers. No custom cryptographic verifier.
- ES256, EdDSA and RS256 software-authenticator registration/login run through real loopback HTTP. Production performs RSA signature **verification only**. RSA signing exists only in test fixtures; future VAPID uses ES256.
- Credential persistence is **not COSE**: `publicKeyState` is canonical base64url of library `StaticState::encode`, paired with mandatory `stateFormat: "webauthn_rp-0.3.0-static"`. Library Decode and key validation reject malformed data; decoded maximum 2058 bytes. Upgrading the verifier requires migration or re-registration, not silently decoding another version.
- Device ID is also the random 16-byte userHandle. First credential requires residentKey; additional credentials use preferred and exclude existing credentials for the RP. BE=0 is a backup policy signal, not attested hardware provenance.
- `rand08 =0.8.6` is test-only; this also resolves RSA's compatible rand 0.8 dependency. Initial 0.8.5 triggered RUSTSEC-2026-0097 and was narrowly upgraded, not ignored.

## Pinned CI tools and exceptions

- `cargo-deny =0.20.2`, `cargo-audit =0.22.2`, installed with `--locked` into a temporary root. Older 0.18.3/0.21.2 cannot parse today's RustSec CVSS 4 advisories.
- The only advisory exception is **RUSTSEC-2023-0071**, `rsa 0.9.10`; verification-only production usage, no private-key operations; VAPID ES256. Owner: **Yuzora maintainers**. Review-by: **2027-03-31**. A contract test enforces version and review date. New private-key operations invalidate the exception.
- Permissive license list includes Unicode-3.0 (attribution), 0BSD, CC0-1.0 and CDLA-Permissive-2.0. OpenSSL is only a permitted license expression for third-party license combinations, never a permitted dependency.
- A narrowly approved license exception permits only `option-ext =0.2.0` / MPL-2.0 through host → dirs → dirs-sys. The crate is unchanged; MPL is file-level copyleft. **Phase 8 must provide license notice and source availability details.** MPL is not globally allowed; a version change requires review.

## BLOCKED: inherited Host database TLS dependencies

Both current tools run locally. Licenses, bans and sources pass; **advisories fail, so Phase 2B supply-chain acceptance is not complete**. CI remains fail-closed; no new advisory ignore was added.

| Advisory | Locked crate | Finding / dependency chain |
|---|---|---|
| RUSTSEC-2025-0134 | rustls-pemfile 1.0.4 | Unmaintained. companion → yuzora-host → vendored tiberius 0.12.3 → rustls-pemfile (also via rustls-native-certs 0.6.3). |
| RUSTSEC-2026-0098 | rustls-webpki 0.101.7 | URI name constraints incorrectly accepted. companion → yuzora-host → tiberius 0.12.3 → tokio-rustls 0.24.1 → rustls 0.21.12 → rustls-webpki. |
| RUSTSEC-2026-0099 | rustls-webpki 0.101.7 | Wildcard DNS name constraints incorrectly accepted. Same chain. |
| RUSTSEC-2026-0104 | rustls-webpki 0.101.7 | Reachable panic parsing CRLs before signature verification. Same chain. |

Parent will separately consider splitting the Host database feature from the Companion dependency, or updating the vendored MSSQL TLS stack. Neither is implemented here; host/desktop source and locks remain untouched. `cargo-audit` reports three vulnerabilities plus one unmaintained warning; deny rejects all four.

Local audit-tool install initially failed in its own AWS-LC build because command-line SDK 27's `arm64e.x1` was not understood by the selected linker. The single approved retry selecting the Xcode clang/SDK succeeded. These tool-build dependencies are not product dependencies; the Companion forbidden-package checks remain clean.

## Phase 3A WebSocket handshake

- Re-enabled axum `ws`; exact `tokio-tungstenite =0.29.0` with only direct `handshake` feature, no native TLS. `futures-util` uses sink/std. The existing axum feature union enables its normal WS dependencies; the lock adds only tungstenite0.29.0, tokio-tungstenite0.29.0 and sha1 0.10.7 (no unrelated version upgrades).
- SHA-1 is the RFC 6455 `Sec-WebSocket-Accept` handshake algorithm, **not** a credential/binding hash or signature algorithm. `tests/contracts.rs` now permits these three packages while continuing to ban OpenSSL, native-tls and every AWS-LC lock entry.
- Hyper is the sole HTTP parser. `tungstenite::handshake::server::create_response_with_body` validates the parsed upgrade headers; Hyper `OnUpgrade` jobs are submitted to the server's bounded queue and existing JoinSet rather than detached axum `on_upgrade` tasks.
- Local cargo-deny licenses/bans/sources passes. Advisories still fails only on the four inherited Host TLS findings above; no new advisory ignore or license exception.
