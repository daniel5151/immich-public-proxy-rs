use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub id: String,
    pub name: String,
    pub value: String,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataSearchRequest {
    pub album_ids: Option<Vec<String>>,
    pub tag_ids: Option<Vec<String>>,
    pub page: Option<u32>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub assets: SearchResponseAssets,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponseAssets {
    pub items: Vec<Asset>,
    pub next_page: Option<String>,
}

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
    pub owner: Option<User>,

    // the proxy augments assets:
    pub password: Option<String>,
    pub key: Option<String>,
    #[serde(default)]
    pub uploader_name: Option<String>,
    #[serde(default)]
    pub uploader_is_fallback: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub album_name: Option<String>,
    pub description: Option<String>,
    pub order: Option<String>, // 'asc' | 'desc'
    pub album_thumbnail_asset_id: Option<String>,
    pub owner: Option<User>,
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
    pub allow_upload: Option<bool>,
    #[serde(default)]
    pub assets: Vec<Asset>,
    pub album: Option<Album>,

    // Proxy augments:
    pub password: Option<String>,
}
