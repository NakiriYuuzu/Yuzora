//! A Space's badge: its initial on a color it keeps. Colors come from
//! `tokens().space_badges`; a Space seen the first time gets the least used
//! one. Pure: what was given out lives in Herdr's shared state
//! (`shared("yuzora.space.colors")`, persisted by the app), passed in as
//! `kept`; the status bar writes the result back, every other caller only
//! reads, so the sidebar, the tab bar, pane headers and other lanes agree.

use serde_json::{Map, Value, json};

/// The palette: `tokens().space_badges`.
pub fn palette() -> Vec<String> {
    yuzora_core::tokens::tokens()["space_badges"]
        .as_array()
        .map(|l| l.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// What a Space's badge says: the first letter or digit of its name, upper case
/// (`~` and other marks only when the name has nothing else).
pub fn initial(name: &str) -> String {
    let first = name.chars().find(|c| c.is_alphanumeric()).or_else(|| name.chars().find(|c| !c.is_whitespace()));
    first.map_or_else(|| "?".to_owned(), |c| c.to_uppercase().collect())
}

/// The color kept for `id`, or the palette's least used one (the first of the
/// ties), added to `kept`. Returns the color and whether `kept` changed.
pub fn assign(kept: &mut Map<String, Value>, id: &str, palette: &[String]) -> (String, bool) {
    if let Some(c) = kept.get(id).and_then(Value::as_str) {
        return (c.to_owned(), false);
    }
    let Some(first) = palette.first() else {
        return ("#4f6bed".to_owned(), false);
    };
    let uses = |c: &String| kept.values().filter(|v| v.as_str() == Some(c)).count();
    let color = palette.iter().min_by_key(|c| uses(c)).unwrap_or(first).clone();
    kept.insert(id.to_owned(), json!(color));
    (color, true)
}

/// Every Space's badge, in `spaces` order (`(id, name)`), from what was `kept`.
/// Returns the badges by id, the new kept map and whether it changed.
pub fn badges(spaces: &[(String, String)], kept: &Value, palette: &[String]) -> (Map<String, Value>, Map<String, Value>, bool) {
    let mut kept = kept.as_object().cloned().unwrap_or_default();
    let mut changed = false;
    let mut out = Map::new();
    for (id, name) in spaces {
        let (color, added) = assign(&mut kept, id, palette);
        changed |= added;
        out.insert(id.clone(), json!({ "initial": initial(name), "color": color }));
    }
    (out, kept, changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colors() -> Vec<String> {
        ["#a", "#b", "#c"].map(String::from).to_vec()
    }

    #[test]
    fn spaces_get_the_least_used_color_and_keep_it() {
        let mut kept = Map::new();
        let p = colors();
        assert_eq!(assign(&mut kept, "s1", &p), ("#a".into(), true));
        assert_eq!(assign(&mut kept, "s2", &p), ("#b".into(), true));
        assert_eq!(assign(&mut kept, "s1", &p), ("#a".into(), false));
        assert_eq!(assign(&mut kept, "s3", &p).0, "#c");
        assert_eq!(assign(&mut kept, "s4", &p).0, "#a");
        kept.remove("s2");
        assert_eq!(assign(&mut kept, "s5", &p).0, "#b");
        assert_eq!(assign(&mut Map::new(), "s1", &[]), ("#4f6bed".into(), false));
    }

    #[test]
    fn initials_skip_marks_and_follow_case() {
        assert_eq!(initial("yuzora"), "Y");
        assert_eq!(initial(".yuuzu"), "Y");
        assert_eq!(initial("~"), "~");
        assert_eq!(initial("中文"), "中");
        assert_eq!(initial("  "), "?");
        assert_eq!(initial("2fa"), "2");
    }

    #[test]
    fn every_caller_gets_the_same_colors_from_the_same_kept_map() {
        let p = colors();
        let spaces = [("s1", "yuzora"), ("s2", "herdr"), ("s3", "infra")].map(|(i, n)| (i.to_owned(), n.to_owned()));
        let kept = json!({ "s2": "#c", "gone": "#a" });
        let (a, new_kept, changed) = badges(&spaces, &kept, &p);
        let (b, _, _) = badges(&spaces, &kept, &p);
        assert_eq!(a, b);
        assert!(changed);
        assert_eq!(a["s1"], json!({ "initial": "Y", "color": "#b" }));
        assert_eq!(a["s2"]["color"], "#c");
        // Spaces not shown now keep their colors (a machine away comes back the same).
        assert_eq!(new_kept["gone"], "#a");
        // Written back, nothing changes any more.
        let (c, _, again) = badges(&spaces, &Value::Object(new_kept), &p);
        assert_eq!(c, a);
        assert!(!again);
        // Nothing kept yet (null, or not an object): colors in order.
        assert_eq!(badges(&spaces, &Value::Null, &p).0["s3"]["color"], "#c");
    }

    #[test]
    fn the_palette_is_the_tokens() {
        let p = palette();
        assert!(p.len() >= 4);
        assert_eq!(p[0], "#4f6bed");
    }
}
