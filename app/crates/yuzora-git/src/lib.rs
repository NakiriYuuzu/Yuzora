//! Yuzora L2（Git）: Git, modelled on IntelliJ IDEA Community: status, staging, commit, branches, log graph, diff and merge.
//!
//! Owned by lane L2. Scripts reach it as `import { … } from "yuzora:git"`;
//! the app registers [`host_module`] for every mod (app/src/yuzora/mod.rs), so
//! a new function only needs adding here, with its line in [`DECLARATIONS`].

use yuzora_core::{HostModule, HostValue};

/// The import specifier: `yuzora:git`.
pub const NAME: &str = "yuzora:git";

/// The TypeScript face of [`host_module`]: one `export function` per function.
pub const DECLARATIONS: &str = r#"
/** The yuzora-git crate's version (Yuzora's). */
export function version(): string;
"#;

/// The `yuzora:git` module.
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
        assert_eq!(m.name(), "yuzora:git");
        assert_eq!(m.call("version", &HostArguments::new([])).unwrap(), HostValue::Str(yuzora_core::VERSION.into()));
    }
}
