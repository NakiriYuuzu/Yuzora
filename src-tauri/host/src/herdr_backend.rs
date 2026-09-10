//! The shared facade keeps validation while the desktop selects the host transport.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HerdrMetadata {
    BinarySource,
    BinaryFingerprint,
    Sessions,
    Status,
    Schema,
}

pub trait HerdrRemoteBackend: Send + Sync {
    fn metadata(&self, query: HerdrMetadata, session: Option<&str>) -> Result<Value, String>;
    fn request(&self, socket: &str, method: &str, params: Value) -> Result<Value, String>;
}
