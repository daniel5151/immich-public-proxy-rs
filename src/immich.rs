use serde::{Deserialize, Serialize};

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
    pub password: Option<String>,
    #[serde(skip)]
    pub key: Option<String>,
    #[serde(skip)]
    pub key_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub album_name: Option<String>,
    pub description: Option<String>,
    pub order: Option<String>, // 'asc' | 'desc'
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
    pub password: Option<String>,
    #[serde(skip)]
    pub key_type: Option<String>,
}

#[cfg(feature = "ssr")]
pub use ssr::*;

#[cfg(feature = "ssr")]
pub mod ssr {
    use reqwest::{Client, Url};

    pub struct ImmichClient {
        pub api_url: String,
        pub http_client: Client,
    }

    impl ImmichClient {
        pub fn new() -> Self {
            let api_url = std::env::var("IMMICH_URL")
                .unwrap_or_else(|_| "http://localhost:2283".to_string())
                .trim_end_matches('/')
                .to_string()
                + "/api";

            static CLIENT: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
            let http_client = CLIENT.get_or_init(Client::new).clone();

            Self {
                api_url,
                http_client,
            }
        }

        pub fn build_url(&self, path: &str, params: &[(&str, &str)]) -> String {
            let url = format!("{}{}", self.api_url, path);
            let mut u = Url::parse(&url).unwrap();
            if !params.is_empty() {
                u.query_pairs_mut()
                    .extend_pairs(params.into_iter().filter(|(_, v)| !v.is_empty()));
            }
            u.to_string()
        }

        pub async fn ping(&self) -> bool {
            self.http_client
                .get(format!("{}/server/ping", self.api_url))
                .send()
                .await
                .is_ok()
        }
    }
}
