//! Yuzora L1（外殼工作區）: Workspace shell: the sidebar's workspace mode, mode bar, tab bar, status bar, the + new pane menu and mixed split tool panes.
//!
//! Owned by lane L1. Scripts reach it as `import { … } from "yuzora:shell"`;
//! the app registers [`host_module`] for every mod (app/src/yuzora/mod.rs), so
//! a new function only needs adding here, with its line in [`DECLARATIONS`].

use yuzora_core::{HostModule, HostValue};

/// The import specifier: `yuzora:shell`.
pub const NAME: &str = "yuzora:shell";

/// The TypeScript face of [`host_module`]: one `export function` per function.
pub const DECLARATIONS: &str = r#"
/** The yuzora-shell crate's version (Yuzora's). */
export function version(): string;
"#;

/// The `yuzora:shell` module.
pub fn host_module() -> HostModule {
    HostModule::new(NAME)
        .declarations(DECLARATIONS)
        .function("version", |_| Ok(HostValue::Str(yuzora_core::VERSION.into())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use yuzora_core::HostArguments;

    #[test]
    fn module_validates_and_reports_its_version() {
        let m = host_module();
        m.validate().unwrap();
        assert_eq!(m.name(), "yuzora:shell");
        assert_eq!(m.call("version", &HostArguments::new([])).unwrap(), HostValue::Str(yuzora_core::VERSION.into()));
    }
}
