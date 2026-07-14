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
    #[serde(default)]
    pub ratio: Option<f32>,
    pub owner_id: Option<String>,
    pub owner: Option<User>,
    pub tags: Option<Vec<Tag>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumUser {
    pub user: User,
    pub role: String, // "owner" | "editor" | "viewer"
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub album_name: Option<String>,
    pub description: Option<String>,
    pub order: Option<String>, // 'asc' | 'desc'
    pub album_thumbnail_asset_id: Option<String>,
    /// Album participants with roles. The owner has `role: "owner"`.
    #[serde(default)]
    pub album_users: Vec<AlbumUser>,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

impl Album {
    /// Returns the album owner from `album_users`.
    pub fn get_owner(&self) -> Option<&User> {
        self.album_users
            .iter()
            .find(|au| au.role == "owner")
            .map(|au| &au.user)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedLink {
    pub key: String,
    pub slug: Option<String>,
    pub description: Option<String>,
    pub expires_at: Option<String>,

    pub r#type: Option<String>, // "ALBUM" or "INDIVIDUAL"
    pub allow_download: Option<bool>,
    pub allow_upload: Option<bool>,
    pub show_metadata: Option<bool>,
    #[serde(default)]
    pub assets: Vec<Asset>,
    pub album: Option<Album>,
    /// Non-null means this shared link is password-protected.
    pub password: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeBucket {
    pub time_bucket: String,
    pub count: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeBucketData {
    pub id: Vec<String>,
    #[serde(default)]
    pub is_image: Vec<bool>,
    #[serde(default)]
    pub file_created_at: Vec<String>,
    #[serde(default)]
    pub owner_id: Vec<String>,
    #[serde(default)]
    pub ratio: Vec<f32>,
}
