//! The crate's own text (errors, prompts, status), in English.

use yuzora_core::i18n::Catalog;

const EN: &str = include_str!("../i18n/en.json");

fn catalog() -> Catalog {
    Catalog::from_json(EN)
}

/// `key`, `{name}` filled from `vars`.
pub fn t(key: &str, vars: &[(&str, String)]) -> String {
    catalog().t(key, vars)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_fill_in() {
        assert_eq!(t("status.verified", &[("host", "lab-01".into()), ("kind", "ED25519".into())]), "lab-01 · ED25519 verified");
    }
}
