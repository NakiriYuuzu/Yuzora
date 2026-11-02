//! The crate's own text (errors, prompts, status), in en and zh-TW, following
//! Yuzora's language setting.

use yuzora_core::i18n::{self, Catalog};

const EN: &str = include_str!("../i18n/en.json");
const ZH_TW: &str = include_str!("../i18n/zh-TW.json");

fn catalog(locale: &str) -> Catalog {
    Catalog::from_json(EN, (locale == i18n::ZH_TW).then_some(ZH_TW))
}

/// `key` in the current language, `{name}` filled from `vars`.
pub fn t(key: &str, vars: &[(&str, String)]) -> String {
    catalog(i18n::current()).t(key, vars)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn keys(text: &str) -> Vec<String> {
        fn walk(prefix: &str, v: &Value, out: &mut Vec<String>) {
            match v {
                Value::Object(m) => m.iter().for_each(|(k, v)| walk(&if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") }, v, out)),
                _ => out.push(prefix.to_owned()),
            }
        }
        let mut out = Vec::new();
        walk("", &serde_json::from_str(text).unwrap(), &mut out);
        out.sort();
        out
    }

    #[test]
    fn both_languages_have_every_key() {
        assert_eq!(keys(EN), keys(ZH_TW));
        assert_eq!(catalog("en").t("status.verified", &[("host", "lab-01".into()), ("kind", "ED25519".into())]), "lab-01 · ED25519 verified");
        assert_eq!(catalog("zh-TW").t("status.verified", &[("host", "lab-01".into()), ("kind", "ED25519".into())]), "lab-01 · ED25519 已驗證");
    }
}
