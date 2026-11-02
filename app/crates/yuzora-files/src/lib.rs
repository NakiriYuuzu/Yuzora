//! Yuzora L5（檔案＋編輯器）: Files and the editor: the file tree, search, watchers, code editor and Markdown preview.
//!
//! Owned by lane L5. Scripts reach it as `import { … } from "yuzora:files"`;
//! the app registers [`host_module`] for every mod (app/src/yuzora/mod.rs), so
//! a new function only needs adding here, with its line in [`DECLARATIONS`].

use yuzora_core::{HostModule, HostValue};

/// The import specifier: `yuzora:files`.
pub const NAME: &str = "yuzora:files";

/// The TypeScript face of [`host_module`]: one `export function` per function.
pub const DECLARATIONS: &str = r#"
/** The yuzora-files crate's version (Yuzora's). */
export function version(): string;
"#;

/// The `yuzora:files` module.
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
        assert_eq!(m.name(), "yuzora:files");
        assert_eq!(m.call("version", &HostArguments::new([])).unwrap(), HostValue::Str(yuzora_core::VERSION.into()));
    }
}
