use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: String,
    pub original_file_name: Option<String>,
    pub original_mime_type: Option<String>,
    pub r#type: String, // "IMAGE" or "VIDEO"
    pub is_trashed: Option<bool>,
    pub db_id: Option<String>,
    pub file_created_at: Option<String>,
    pub exif_info: Option<serde_json::Value>,
    pub width: Option<i32>,
    pub height: Option<i32>,

    // the proxy augments assets:
    #[serde(skip)]
    #[allow(dead_code)]
    pub password: Option<String>,
    #[serde(skip)]
    #[allow(dead_code)]
    pub key: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub album_name: Option<String>,
    pub description: Option<String>,
    pub order: Option<String>, // 'asc' | 'desc'
    pub album_thumbnail_asset_id: Option<String>,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedLink {
    pub key: String,
    pub description: Option<String>,
    pub expires_at: Option<String>,
    #[serde(default)]
    pub password_required: bool,
    pub r#type: Option<String>, // "ALBUM" or "INDIVIDUAL"
    pub allow_download: Option<bool>,
    #[serde(default)]
    pub assets: Vec<Asset>,
    pub album: Option<Album>,

    // Proxy augments:
    #[serde(skip)]
    #[allow(dead_code)]
    pub password: Option<String>,
}
