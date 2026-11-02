//! The crate's own strings (the grid, the editor, error messages):
//! `i18n/en.json` and `i18n/zh-TW.json`, in the language Yuzora shows.

use std::sync::OnceLock;
use yuzora_core::i18n::{Catalog, EN, current};

const EN_JSON: &str = include_str!("../i18n/en.json");
const ZH_TW_JSON: &str = include_str!("../i18n/zh-TW.json");

fn catalog(locale: &str) -> &'static Catalog {
    static EN_CAT: OnceLock<Catalog> = OnceLock::new();
    static ZH_CAT: OnceLock<Catalog> = OnceLock::new();
    if locale == EN {
        EN_CAT.get_or_init(|| Catalog::from_json(EN_JSON, None))
    } else {
        ZH_CAT.get_or_init(|| Catalog::from_json(EN_JSON, Some(ZH_TW_JSON)))
    }
}

/// `key` in the shown language, `{name}` filled from `vars`.
pub fn t(key: &str, vars: &[(&str, String)]) -> String {
    catalog(current()).t(key, vars)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(json: &str) -> Vec<String> {
        fn walk(prefix: &str, v: &serde_json::Value, out: &mut Vec<String>) {
            match v {
                serde_json::Value::Object(m) => {
                    for (k, v) in m {
                        let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                        walk(&key, v, out);
                    }
                }
                _ => out.push(prefix.to_owned()),
            }
        }
        let mut out = Vec::new();
        walk("", &serde_json::from_str(json).unwrap(), &mut out);
        out.sort();
        out
    }

    #[test]
    fn both_languages_have_the_same_keys() {
        assert_eq!(keys(EN_JSON), keys(ZH_TW_JSON));
    }

    #[test]
    fn variables_fill_in() {
        let zh = catalog("zh-TW");
        assert_eq!(zh.t("grid.modified", &[("count", "2".into())]), "2 個儲存格已修改");
    }
}
