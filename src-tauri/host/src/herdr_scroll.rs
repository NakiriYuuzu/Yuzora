//! Official protocol-22 scroll state; terminal frame bytes are not scrollback.
use crate::herdr_service::HerdrManager;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all(serialize = "camelCase"))]
pub struct HerdrPaneScrollInfo {
    pub offset_from_bottom: u64,
    pub max_offset_from_bottom: u64,
    pub viewport_rows: u64,
}

fn parse_scroll(
    response: serde_json::Value,
    pane_id: &str,
) -> Result<Option<HerdrPaneScrollInfo>, String> {
    let pane = &response["result"]["pane"];
    if response["result"]["type"] != "pane_info" || pane["pane_id"].as_str() != Some(pane_id) {
        return Err("pane.get returned a different pane".into());
    }
    if pane["scroll"].is_null() {
        return Ok(None);
    }
    let scroll: HerdrPaneScrollInfo =
        serde_json::from_value(pane["scroll"].clone()).map_err(|error| error.to_string())?;
    if scroll.offset_from_bottom > scroll.max_offset_from_bottom
        || scroll.viewport_rows == 0
        || scroll.max_offset_from_bottom > 9_007_199_254_740_991
        || scroll.viewport_rows > 9_007_199_254_740_991
    {
        return Err("invalid HERDR pane scroll range".into());
    }
    Ok(Some(scroll))
}

impl HerdrManager {
    pub fn pane_scroll_state(
        &self,
        session: Option<&str>,
        pane_id: String,
    ) -> Result<Option<HerdrPaneScrollInfo>, String> {
        if pane_id.trim().is_empty() {
            return Err("pane_id is required".into());
        }
        let response = self.call_checked_api(
            session,
            |api| api.snapshot && api.methods.iter().any(|method| method == "pane.get"),
            "pane.get",
            serde_json::json!({"pane_id": pane_id}),
            "herdr pane.get unavailable",
        )?;
        parse_scroll(response, &pane_id)
    }

    pub fn pane_scroll_to(
        &self,
        session: Option<&str>,
        pane_id: String,
        offset: u64,
    ) -> Result<Option<HerdrPaneScrollInfo>, String> {
        if pane_id.trim().is_empty() || offset > 9_007_199_254_740_991 {
            return Err("invalid pane scroll target".into());
        }
        self.call_checked_api(
            session,
            |api| api.snapshot && api.methods.iter().any(|method| method == "pane.scroll"),
            "pane.scroll",
            serde_json::json!({"pane_id": pane_id, "offset_from_bottom": offset}),
            "herdr pane.scroll unavailable",
        )?;
        self.pane_scroll_state(session, pane_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_only_official_ranges_and_exact_pane_identity() {
        let response = serde_json::json!({"result":{"type":"pane_info","pane":{"pane_id":"w1:p1","scroll":{"offset_from_bottom":20,"max_offset_from_bottom":100,"viewport_rows":24}}}});
        assert_eq!(
            parse_scroll(response.clone(), "w1:p1")
                .unwrap()
                .unwrap()
                .max_offset_from_bottom,
            100
        );
        assert!(parse_scroll(response, "w2:p1").is_err());
        assert_eq!(
            parse_scroll(
                serde_json::json!({"result":{"type":"pane_info","pane":{"pane_id":"w1:p1"}}}),
                "w1:p1"
            )
            .unwrap(),
            None
        );
    }
    #[test]
    fn accepts_installed_protocol22_snake_case_scroll_metadata() {
        // Minimized values from a readonly installed 0.9.0 pane inventory.
        let response = serde_json::json!({"result":{"type":"pane_info","pane":{"pane_id":"fixture-pane","scroll":{"max_offset_from_bottom":715,"offset_from_bottom":0,"viewport_rows":53}}}});
        let parsed = parse_scroll(response, "fixture-pane").unwrap().unwrap();
        assert_eq!(
            serde_json::to_value(parsed).unwrap(),
            serde_json::json!({"maxOffsetFromBottom":715,"offsetFromBottom":0,"viewportRows":53})
        );
    }

    #[test]
    fn rejects_invalid_ranges_and_serializes_the_typed_boundary() {
        let invalid = serde_json::json!({"result":{"type":"pane_info","pane":{"pane_id":"p","scroll":{"offset_from_bottom":101,"max_offset_from_bottom":100,"viewport_rows":24}}}});
        assert!(parse_scroll(invalid, "p").is_err());
        let value = serde_json::to_value(HerdrPaneScrollInfo {
            offset_from_bottom: 0,
            max_offset_from_bottom: 100,
            viewport_rows: 24,
        })
        .unwrap();
        assert_eq!(value["offsetFromBottom"], 0);
        assert_eq!(value["maxOffsetFromBottom"], 100);
    }
}
