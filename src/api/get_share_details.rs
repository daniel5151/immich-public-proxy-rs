use crate::immich_client::model::SharedLink;

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
                    allow_upload: None,
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
                        if let Some(ref admin_api_key) = client.admin_api_key {
                            let tags_url = client.build_url("/tags", &[]);
                            if let Ok(tags_res) = client
                                .http_client
                                .get(&tags_url)
                                .header("x-api-key", admin_api_key)
                                .send()
                                .await
                            {
                                if let Ok(tags) = tags_res
                                    .json::<Vec<crate::immich_client::model::Tag>>()
                                    .await
                                {
                                    let shared_by_tag = tags
                                        .iter()
                                        .find(|t| t.name == "SharedBy" && t.parent_id.is_none());

                                    if let Some(parent) = shared_by_tag {
                                        for tag in tags
                                            .iter()
                                            .filter(|t| t.parent_id.as_ref() == Some(&parent.id))
                                        {
                                            let username = tag.name.clone();
                                            let mut page = 1u32;
                                            let search_url =
                                                client.build_url("/search/metadata", &[]);
                                            loop {
                                                let search_req = crate::immich_client::model::MetadataSearchRequest {
                                                    album_ids: Some(vec![album.id.clone()]),
                                                    tag_ids: Some(vec![tag.id.clone()]),
                                                    page: Some(page),
                                                };
                                                match client
                                                    .http_client
                                                    .post(&search_url)
                                                    .header("x-api-key", admin_api_key)
                                                    .json(&search_req)
                                                    .send()
                                                    .await
                                                    .and_then(|r| Ok(r))
                                                {
                                                    Ok(search_res) => {
                                                        if let Ok(search_data) = search_res.json::<crate::immich_client::model::SearchResponse>().await {
                                                            let has_next = search_data.assets.next_page.is_some();
                                                            let tagged_asset_ids: std::collections::HashSet<_> = search_data.assets.items.into_iter().map(|a| a.id).collect();
                                                            for asset in &mut album_data.assets {
                                                                if tagged_asset_ids.contains(&asset.id) {
                                                                    asset.uploader_name = Some(username.clone());
                                                                }
                                                            }
                                                            if has_next { page += 1; } else { break; }
                                                        } else { break; }
                                                    }
                                                    Err(_) => break,
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Fetch owner info per-asset via admin key (search/metadata doesn't return owner)
                        if let Some(ref admin_api_key) = client.admin_api_key {
                            for asset in &mut album_data.assets {
                                asset.key = Some(key.clone());
                                asset.password = password.clone();
                                if asset.uploader_name.is_none() {
                                    let asset_url =
                                        client.build_url(&format!("/assets/{}", asset.id), &[]);
                                    if let Ok(res) = client
                                        .http_client
                                        .get(&asset_url)
                                        .header("x-api-key", admin_api_key)
                                        .send()
                                        .await
                                    {
                                        if let Ok(full_asset) =
                                            res.json::<crate::immich_client::model::Asset>().await
                                        {
                                            asset.uploader_name =
                                                full_asset.owner.as_ref().map(|o| o.name.clone());
                                            asset.uploader_is_fallback = true;
                                        }
                                    }
                                }
                            }
                        } else {
                            for asset in &mut album_data.assets {
                                asset.key = Some(key.clone());
                                asset.password = password.clone();
                            }
                        }

                        link.assets = album_data.assets.clone();
                        link.album = Some(album_data);
                    }
                }
            }
        } else {
            for asset in &mut link.assets {
                asset.key = Some(key.clone());
                asset.password = password.clone();
                if asset.uploader_name.is_none() {
                    if let Some(ref admin_api_key) = client.admin_api_key {
                        let asset_url = client.build_url(&format!("/assets/{}", asset.id), &[]);
                        if let Ok(res) = client
                            .http_client
                            .get(&asset_url)
                            .header("x-api-key", admin_api_key)
                            .send()
                            .await
                        {
                            if let Ok(full_asset) =
                                res.json::<crate::immich_client::model::Asset>().await
                            {
                                asset.uploader_name =
                                    full_asset.owner.as_ref().map(|o| o.name.clone());
                                asset.uploader_is_fallback = true;
                            }
                        }
                    }
                }
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

        // If all assets have the exact same uploader, omit the badges
        if !link.assets.is_empty() {
            let first_uploader = link.assets[0].uploader_name.clone();
            let all_same = link
                .assets
                .iter()
                .all(|a| a.uploader_name == first_uploader);

            if all_same {
                for asset in &mut link.assets {
                    asset.uploader_name = None;
                }
                if let Some(ref mut album) = link.album {
                    for asset in &mut album.assets {
                        asset.uploader_name = None;
                    }
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
