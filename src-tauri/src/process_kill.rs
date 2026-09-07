//! Process ownership primitives shared with the host helper.
pub use yuzora_host::process_kill::*;

#[cfg(test)]
mod tests {
    #[test]
    fn windows_shell_callers_use_the_shared_launchers() {
        for (name, source, launcher) in [
            (
                "process_service.rs",
                include_str!("../host/src/process_service.rs"),
                "windows_shell_command(",
            ),
            (
                "lsp_download.rs",
                include_str!("../host/src/lsp_download.rs"),
                "windows_batch_command(",
            ),
        ] {
            assert!(
                !source.contains(r#"args(["/C""#),
                "{name} 又出現未經 raw_arg 的 cmd.exe launcher（#35）"
            );
            assert!(
                source.contains(launcher),
                "{name} 必須走共用的 {launcher}（#35）"
            );
        }
    }
}
