use crate::immich_client::model::SharedLink;

#[cfg(feature = "ssr")]
use crate::immich_client::client::ImmichClient;
#[cfg(feature = "ssr")]
use crate::immich_client::model::Asset;
use leptos::prelude::*;
use serde::Deserialize;
use serde::Serialize;

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

    let client = ImmichClient::new();
    let (status, text) = client.fetch_share_me(&key, password.as_deref()).await?;

    if status == 401 {
        return server_helpers::handle_unauthorized(&client, &key, public_base_url).await;
    }

    if !status.is_success() {
        eprintln!("fetch_share_me failed for key '{}': {} — {}", key, status, text);
        return Err(ServerFnError::new(format!(
            "Failed with status: {}",
            status
        )));
    }

    let mut link: SharedLink =
        serde_json::from_str(&text).map_err(|e| ServerFnError::new(e.to_string()))?;
    link.password = password.clone();

    let allow_download = link.allow_download.unwrap_or(false);
    let show_metadata = link.show_metadata.unwrap_or(true);

    // Populate album assets if it's an album share
    if link.r#type.as_deref() == Some("ALBUM") {
        if let Some(ref album) = link.album {
            let mut album_params = vec![("key", link.key.as_str())];
            if let Some(p) = &password {
                album_params.push(("password", p.as_str()));
            }

            let album_url = client.build_url(&format!("/albums/{}", album.id), &album_params);
            let album_res = client.http_client.get(&album_url).send().await?;

            if !album_res.status().is_success() {
                let status = album_res.status();
                let body = album_res.text().await.unwrap_or_default();
                eprintln!(
                    "Failed to fetch album {}: {} — {}",
                    album.id, status, body
                );
                if status == 403 {
                    return Err(ServerFnError::new(
                        "Permission denied fetching album. Ensure the shared link key has the 'album.read' permission.",
                    ));
                }
                return Err(ServerFnError::new(format!(
                    "Failed to fetch album: {}",
                    status
                )));
            }

            if let Ok(mut album_data) =
                album_res.json::<crate::immich_client::model::Album>().await
            {
                if show_metadata {
                    server_helpers::resolve_uploader_names(
                        &client,
                        &album.id,
                        &mut album_data.assets,
                    )
                    .await;
                }

                server_helpers::stamp_asset_credentials(
                    &mut album_data.assets,
                    &key,
                    &password,
                );
                link.assets = album_data.assets.clone();
                link.album = Some(album_data);
            }
        }
    } else {
        if show_metadata {
            server_helpers::resolve_owner_fallback(&client, &mut link.assets).await;
        }
        server_helpers::stamp_asset_credentials(&mut link.assets, &key, &password);
    }

    // Sort album assets if there is a sort order specified
    if let Some(ref album) = link.album {
        match album.order.as_deref() {
            Some("asc") => link
                .assets
                .sort_by(|a, b| a.file_created_at.cmp(&b.file_created_at)),
            Some("desc") => link
                .assets
                .sort_by(|a, b| b.file_created_at.cmp(&a.file_created_at)),
            _ => {}
        }
    }

    // If all assets have the exact same uploader, omit the badges
    server_helpers::strip_uniform_uploader(&mut link);

    // Stamp download_url server-side when downloads are allowed
    if allow_download {
        for asset in &mut link.assets {
            if let Some(ref k) = asset.key {
                asset.download_url = Some(format!("/share/photo/{}/{}/original", k, asset.id));
            }
        }
    }

    Ok(ShareDetails {
        link,
        password_required: false,
        public_base_url,
        request_key: key,
    })
}

#[cfg(feature = "ssr")]
mod server_helpers {
    use super::*;

    /// Handles a 401 response by consulting the admin API to determine
    /// whether the share requires a password or is simply invalid.
    pub async fn handle_unauthorized(
        client: &ImmichClient,
        key: &str,
        public_base_url: String,
    ) -> Result<ShareDetails, ServerFnError> {
        match client.get_admin_shared_link(key).await {
            Ok(Some(link)) if link.password.is_some() => {
                eprintln!("Share '{}' requires a password", key);
                Ok(password_required_response(key, public_base_url))
            }
            Ok(Some(_)) => {
                eprintln!("Share '{}' returned 401 but has no password set — share is not accessible", key);
                Err(ServerFnError::ServerError(
                    "Share is not accessible".to_string(),
                ))
            }
            Ok(None) if client.admin_api_key.is_some() => {
                // Admin API is available but the link wasn't found
                eprintln!("Share '{}' not found via admin API — invalid share key", key);
                Err(ServerFnError::ServerError("Invalid share key".to_string()))
            }
            _ => {
                // No admin key or API error — can't determine the cause,
                // so assume password required as a safe fallback
                eprintln!(
                    "Share '{}' returned 401 and admin API is unavailable — assuming password required",
                    key
                );
                Ok(password_required_response(key, public_base_url))
            }
        }
    }

    /// Builds a `ShareDetails` indicating a password is required.
    fn password_required_response(key: &str, public_base_url: String) -> ShareDetails {
        ShareDetails {
            link: SharedLink {
                key: key.to_string(),
                slug: None,
                description: None,
                expires_at: None,
                password_required: true,
                r#type: None,
                allow_download: None,
                allow_upload: None,
                show_metadata: None,
                assets: vec![],
                album: None,
                password: None,
            },
            password_required: true,
            public_base_url,
            request_key: key.to_string(),
        }
    }

    /// Sets `key` and `password` on every asset.
    pub fn stamp_asset_credentials(assets: &mut [Asset], key: &str, password: &Option<String>) {
        for asset in assets.iter_mut() {
            asset.key = Some(key.to_string());
            asset.password = password.clone();
        }
    }

    /// If every asset has the exact same uploader name, clear all badges
    /// (since a uniform badge adds no information).
    pub fn strip_uniform_uploader(link: &mut SharedLink) {
        if link.assets.is_empty() {
            return;
        }

        let first = link.assets[0].uploader_name.clone();
        let all_same = link.assets.iter().all(|a| a.uploader_name == first);
        if !all_same {
            return;
        }

        for asset in &mut link.assets {
            asset.uploader_name = None;
        }
        if let Some(ref mut album) = link.album {
            for asset in &mut album.assets {
                asset.uploader_name = None;
            }
        }
    }

    /// Resolves uploader names for album assets using `SharedBy/` tags,
    /// then falls back to asset owner for any that remain unresolved.
    pub async fn resolve_uploader_names(
        client: &ImmichClient,
        album_id: &str,
        assets: &mut [Asset],
    ) {
        resolve_shared_by_tags(client, album_id, assets).await;
        resolve_owner_fallback(client, assets).await;
    }

    /// Looks up `SharedBy/{name}` tags via the admin API and stamps matching
    /// assets with the tag name as `uploader_name`.
    async fn resolve_shared_by_tags(client: &ImmichClient, album_id: &str, assets: &mut [Asset]) {
        let Some(tags_res): Option<reqwest::Response> = client.admin_get("/tags").await else {
            return;
        };
        if !tags_res.status().is_success() {
            static WARN_ONCE: std::sync::Once = std::sync::Once::new();
            WARN_ONCE.call_once(|| {
                eprintln!(
                    "warning: Admin API /tags failed: {} — uploader attribution via SharedBy/ tags will be unavailable",
                    tags_res.status()
                );
            });
            return;
        }
        let Ok(tags) = tags_res
            .json::<Vec<crate::immich_client::model::Tag>>()
            .await
        else {
            return;
        };

        let Some(parent) = tags
            .iter()
            .find(|t| t.name == "SharedBy" && t.parent_id.is_none())
        else {
            return;
        };

        let child_tags: Vec<_> = tags
            .iter()
            .filter(|t| t.parent_id.as_ref() == Some(&parent.id))
            .collect();

        for tag in child_tags {
            let username = &tag.name;
            let mut page = 1u32;

            loop {
                let search_req = crate::immich_client::model::MetadataSearchRequest {
                    album_ids: Some(vec![album_id.to_string()]),
                    tag_ids: Some(vec![tag.id.clone()]),
                    page: Some(page),
                };

                let Some(search_res): Option<reqwest::Response> =
                    client.admin_post("/search/metadata", &search_req).await
                else {
                    break;
                };
                let Ok(search_data) = search_res
                    .json::<crate::immich_client::model::SearchResponse>()
                    .await
                else {
                    break;
                };

                let has_next = search_data.assets.next_page.is_some();
                let tagged_ids: std::collections::HashSet<_> =
                    search_data.assets.items.into_iter().map(|a| a.id).collect();

                for asset in assets.iter_mut() {
                    if tagged_ids.contains(&asset.id) {
                        asset.uploader_name = Some(username.clone());
                    }
                }

                if has_next {
                    page += 1;
                } else {
                    break;
                }
            }
        }
    }

    /// For assets that still have no `uploader_name`, fetches the asset's
    /// owner via the admin API and uses the owner name as a fallback.
    pub async fn resolve_owner_fallback(client: &ImmichClient, assets: &mut [Asset]) {
        for asset in assets.iter_mut() {
            if asset.uploader_name.is_some() {
                continue;
            }
            let Some(res): Option<reqwest::Response> =
                client.admin_get(&format!("/assets/{}", asset.id)).await
            else {
                continue;
            };

            if !res.status().is_success() {
                static WARN_ONCE: std::sync::Once = std::sync::Once::new();
                WARN_ONCE.call_once(|| {
                    eprintln!(
                        "warning: Admin API /assets/{{id}} failed: {} — owner name fallback will be unavailable",
                        res.status()
                    );
                });
                continue;
            }

            if let Ok(full_asset) = res.json::<Asset>().await {
                asset.uploader_name = full_asset.owner.as_ref().map(|o| o.name.clone());
                asset.uploader_is_fallback = true;
            }
        }
    }
}
