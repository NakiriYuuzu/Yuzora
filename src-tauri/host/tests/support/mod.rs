// Allows the same process-level tests to run against cross-built artifacts on
// WSL/SSH without requiring a Rust toolchain on the target host.
pub fn helper_binary() -> std::path::PathBuf {
    std::env::var_os("YUZORA_HOST_TEST_BINARY")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_yuzora-host").into())
}
