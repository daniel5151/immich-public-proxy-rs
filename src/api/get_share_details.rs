use crate::immich_client::model::SharedLink;

use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ShareDetails {
    pub link: SharedLink,
    pub password_required: bool,
    pub public_base_url: String,
    pub request_key: String,
}

#[server(GetShareDetails, "/api")]
pub async fn get_share_details(
    key: String,
    password: Option<String>,
) -> Result<ShareDetails, ServerFnError> {
    let headers = leptos_axum::extract::<axum::http::HeaderMap>()
        .await
        .map_err(|_| ServerFnError::new("Failed to extract headers"))?;

    let host = headers
        .get("host")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| ServerFnError::new("Host header must be present"))?;

    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|p| p.to_str().ok())
        .unwrap_or("http");

    let public_base_url =
        std::env::var("PUBLIC_BASE_URL").unwrap_or_else(|_| format!("{}://{}", proto, host));

    // Check cookie for password if not provided
    let password =
        password.or_else(|| crate::immich_client::client::get_cookie_password(&headers, &key));

    let client = crate::immich_client::client::ImmichClient::new();
    let (status, text) = client.fetch_share_me(&key, password.as_deref()).await?;

    if status == 401 {
        // Assume password required
        if text.contains("Invalid password") || text.contains("Invalid share key") {
            return Ok(ShareDetails {
                link: SharedLink {
                    key: key.clone(),
                    description: None,
                    expires_at: None,
                    password_required: true,
                    r#type: None,
                    allow_download: None,
                    assets: vec![],
                    album: None,
                    password: None,
                },
                password_required: true,
                public_base_url,
                request_key: key,
            });
        }
        return Err(ServerFnError::new("Unauthorized/Unknown"));
    } else if status.is_success() {
        let mut link: SharedLink =
            serde_json::from_str(&text).map_err(|e| ServerFnError::new(e.to_string()))?;
        link.password = password.clone();

        // Populate album assets if it's an album
        if link.r#type.as_deref() == Some("ALBUM") {
            if let Some(ref album) = link.album {
                let mut album_params = vec![("key", link.key.as_str())];
                if let Some(p) = &password {
                    album_params.push(("password", p.as_str()));
                }

                let album_url = client.build_url(&format!("/albums/{}", album.id), &album_params);
                let album_res = client.http_client.get(&album_url).send().await?;
                if album_res.status().is_success() {
                    if let Ok(mut album_data) =
                        album_res.json::<crate::immich_client::model::Album>().await
                    {
                        for asset in &mut album_data.assets {
                            asset.key = Some(key.clone());
                            asset.password = password.clone();
                        }
                        link.assets = album_data.assets;
                    }
                }
            }
        } else {
            for asset in &mut link.assets {
                asset.key = Some(key.clone());
                asset.password = password.clone();
            }
        }

        // Sort album if there is a sort order specified
        if let Some(ref album) = link.album {
            if let Some(ref order) = album.order {
                if order == "asc" {
                    link.assets
                        .sort_by(|a, b| a.file_created_at.cmp(&b.file_created_at));
                } else if order == "desc" {
                    link.assets
                        .sort_by(|a, b| b.file_created_at.cmp(&a.file_created_at));
                }
            }
        }

        return Ok(ShareDetails {
            link,
            password_required: false,
            public_base_url,
            request_key: key,
        });
    }

    Err(ServerFnError::new(format!(
        "Failed with status: {}",
        status
    )))
}
