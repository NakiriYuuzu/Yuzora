//! The crate's own strings (the grid, the editor, error messages):
//! `i18n/en.json`.

use std::sync::OnceLock;
use yuzora_core::i18n::Catalog;

const EN_JSON: &str = include_str!("../i18n/en.json");

fn catalog() -> &'static Catalog {
    static CAT: OnceLock<Catalog> = OnceLock::new();
    CAT.get_or_init(|| Catalog::from_json(EN_JSON))
}

/// `key` in the crate's strings, `{name}` filled from `vars`.
pub fn t(key: &str, vars: &[(&str, String)]) -> String {
    catalog().t(key, vars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_fill_in() {
        assert_eq!(catalog().t("grid.modified", &[("count", "2".into())]), "2 cells modified");
    }
}
