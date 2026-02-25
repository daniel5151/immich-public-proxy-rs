use crate::immich::SharedLink;
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ShareDetails {
    pub link: SharedLink,
    pub password_required: bool,
    pub public_base_url: String,
}

#[server(GetShareDetails, "/api")]
pub async fn get_share_details(
    key: String,
    password: Option<String>,
) -> Result<ShareDetails, ServerFnError> {
    use crate::immich::ImmichClient;

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
    let password = password.or_else(|| crate::immich::get_cookie_password(&headers, &key));

    let client = ImmichClient::new();
    let params = if let Some(p) = &password {
        vec![("key", key.as_str()), ("password", p.as_str())]
    } else {
        vec![("key", key.as_str())]
    };

    let url = client.build_url("/shared-links/me", &params);
    let res = client.http_client.get(&url).send().await?;

    if res.status() == 401 {
        // Assume password required
        if let Ok(json) = res.json::<serde_json::Value>().await {
            if json.get("message").and_then(|m| m.as_str()) == Some("Invalid password") {
                return Ok(ShareDetails {
                    link: SharedLink {
                        key,
                        description: None,
                        expires_at: None,
                        password_required: true,
                        r#type: None,
                        allow_download: None,
                        assets: vec![],
                        album: None,
                        password: None,
                        key_type: None,
                    },
                    password_required: true,
                    public_base_url,
                });
            }
        }
        return Err(ServerFnError::new("Unauthorized/Unknown"));
    } else if res.status().is_success() {
        let mut link: SharedLink = res.json().await?;
        link.password = password.clone();

        // Populate album assets if it's an album
        if link.r#type.as_deref() == Some("ALBUM") {
            if let Some(ref album) = link.album {
                let album_url = client.build_url(&format!("/albums/{}", album.id), &params);
                let album_res = client.http_client.get(&album_url).send().await?;
                if album_res.status().is_success() {
                    if let Ok(mut album_data) = album_res.json::<crate::immich::Album>().await {
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
        });
    }

    Err(ServerFnError::new(format!(
        "Failed with status: {}",
        res.status()
    )))
}
