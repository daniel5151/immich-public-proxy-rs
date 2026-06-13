use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../frontend/src/types/generated/")]
#[allow(dead_code)]
pub struct SafeAsset {
    pub id: String,
    pub original_file_name: Option<String>,
    pub r#type: String, // "IMAGE" or "VIDEO"
    pub file_created_at: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub uploader_name: Option<String>,
    #[serde(default)]
    pub uploader_is_fallback: bool,
    #[serde(skip_serializing)]
    #[ts(skip)]
    pub owner_id: Option<String>,
    pub download_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../frontend/src/types/generated/")]
#[allow(dead_code)]
pub struct SafeAlbum {
    pub id: String,
    pub album_name: Option<String>,
    pub description: Option<String>,
    pub album_thumbnail_asset_id: Option<String>,
    #[serde(skip_serializing, default)]
    #[ts(skip)]
    pub assets: Vec<SafeAsset>,
}

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../frontend/src/types/generated/")]
pub struct SafeSharedLink {
    pub key: String,
    pub description: Option<String>,
    pub r#type: Option<String>,
    pub allow_download: Option<bool>,
    pub allow_upload: Option<bool>,
    #[serde(default)]
    pub assets: Vec<SafeAsset>,
    pub album: Option<SafeAlbum>,
}

impl SafeAsset {
    pub fn from_base(asset: crate::immich_client::model::Asset) -> Self {
        SafeAsset {
            id: asset.id,
            original_file_name: asset.original_file_name,
            r#type: asset.r#type,
            file_created_at: asset.file_created_at,
            width: asset.width,
            height: asset.height,
            uploader_name: None,
            uploader_is_fallback: false,
            owner_id: asset.owner_id,
            download_url: None,
        }
    }
}

impl SafeAlbum {
    pub fn from_base(album: crate::immich_client::model::Album) -> Self {
        SafeAlbum {
            id: album.id,
            album_name: album.album_name,
            description: album.description,
            album_thumbnail_asset_id: album.album_thumbnail_asset_id,
            assets: album.assets.into_iter().map(SafeAsset::from_base).collect(),
        }
    }
}

impl SafeSharedLink {
    pub fn from_base(link: crate::immich_client::model::SharedLink) -> Self {
        let upload_key_set = std::env::var("IMMICH_API_KEY_UPLOAD_USER").is_ok();
        SafeSharedLink {
            key: link.key,
            description: link.description,
            r#type: link.r#type,
            allow_download: link.allow_download,
            allow_upload: if upload_key_set {
                link.allow_upload
            } else {
                Some(false)
            },
            assets: link.assets.into_iter().map(SafeAsset::from_base).collect(),
            album: link.album.map(SafeAlbum::from_base),
        }
    }
}
