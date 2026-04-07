use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuratedListEntry {
    pub provider_id: String,
    pub descriptor_hash: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CuratedListPayload {
    pub schema_version: String,
    pub list_type: String,
    pub curator_id: String,
    pub list_id: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub entries: Vec<CuratedListEntry>,
}
