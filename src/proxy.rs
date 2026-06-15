use crate::immich_client::client::ImmichClient;
use crate::immich_client::client::get_cookie_password;
use axum::body::Body;
use axum::extract::Form;
use axum::extract::Path;
use axum::extract::Query;
use axum::extract::Request;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Redirect;
use serde::Deserialize;

pub trait ProxyRoutes {
    fn proxy_routes(self) -> Self;
}

impl<T: Clone + Send + Sync + 'static> ProxyRoutes for axum::Router<T> {
    fn proxy_routes(self) -> Self {
        self.route(
            "/share/photo/{key}/{id}/{size}",
            axum::routing::get(proxy_photo),
        )
        .route(
            "/share/photo/{key}/{id}",
            axum::routing::get(proxy_photo_no_size),
        )
        .route("/share/video/{key}/{id}", axum::routing::get(proxy_video))
        .route("/share/unlock", axum::routing::post(unlock_share_handler))
        .route("/share/{key}/download", axum::routing::get(download_all))
        .route(
            "/share/{key}/upload",
            axum::routing::post(upload_asset_handler),
        )
        .route(
            "/share/{key}/status/{asset_id}",
            axum::routing::get(upload_status_handler),
        )
    }
}

#[derive(Deserialize)]
pub struct UnlockPayload {
    key: String,
    password: String,
}

fn is_safe_param(s: &str) -> bool {
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Validate that the request origin matches the host, for CSRF protection.
/// Returns true if the request passes the CSRF check (i.e., is same-origin).
fn check_csrf(headers: &HeaderMap) -> bool {
    // Prefer the Sec-Fetch-Site header (set by modern browsers, unforgeable)
    if let Some(site) = headers.get("sec-fetch-site") {
        return site == "same-origin";
    }

    // Fallback: compare parsed Origin host against the Host header
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok());

    match (origin, host) {
        (Some(o), Some(h)) => {
            // Parse the Origin as a URI and extract its authority (host:port)
            match o.parse::<axum::http::Uri>() {
                Ok(uri) => match uri.authority() {
                    Some(auth) => auth.as_str() == h,
                    None => false,
                },
                Err(_) => false,
            }
        }
        // If neither header is present, we can't validate - deny by default
        _ => false,
    }
}

pub async fn unlock_share_handler(
    headers: HeaderMap,
    Form(payload): Form<UnlockPayload>,
) -> impl IntoResponse {
    if !is_safe_param(&payload.key) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    if !check_csrf(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }

    let client = ImmichClient::new();
    let mut success = false;
    let mut real_key = payload.key.clone();

    if let Ok((status, text)) = client
        .fetch_share_me(&payload.key, Some(&payload.password))
        .await
    {
        if status.is_success() {
            success = true;
            if let Ok(link) = serde_json::from_str::<crate::immich_client::model::SharedLink>(&text)
            {
                real_key = link.key;
            }
        }
    }

    if success {
        use base64::Engine;
        let b64_pwd = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&payload.password);
        let b64_key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&payload.key);

        let is_https = headers
            .get("x-forwarded-proto")
            .and_then(|p| p.to_str().ok())
            .map(|p| p.eq_ignore_ascii_case("https"))
            .unwrap_or(false);
        let secure_flag = if is_https { " Secure;" } else { "" };

        let cookie1 = format!(
            "immich_pwd_{}={};{} Path=/; HttpOnly; SameSite=Lax",
            b64_key, b64_pwd, secure_flag
        );
        let mut resp = Redirect::to(&format!("/share/{}", payload.key)).into_response();
        resp.headers_mut()
            .append(axum::http::header::SET_COOKIE, cookie1.parse().unwrap());

        if payload.key != real_key {
            let b64_real_key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&real_key);
            let cookie2 = format!(
                "immich_pwd_{}={};{} Path=/; HttpOnly; SameSite=Lax",
                b64_real_key, b64_pwd, secure_flag
            );
            resp.headers_mut()
                .append(axum::http::header::SET_COOKIE, cookie2.parse().unwrap());
        }
        return resp;
    }
    Redirect::to(&format!("/share/{}", payload.key)).into_response()
}

pub async fn proxy_photo(
    headers: HeaderMap,
    Path((key, id, size)): Path<(String, String, String)>,
) -> impl IntoResponse {
    proxy_photo_impl(headers, key, id, size).await
}

pub async fn proxy_photo_no_size(
    headers: HeaderMap,
    Path((key, id)): Path<(String, String)>,
) -> impl IntoResponse {
    proxy_photo_impl(headers, key, id, "preview".to_string()).await
}

async fn proxy_photo_impl(
    headers: HeaderMap,
    key: String,
    id: String,
    size_str: String,
) -> impl IntoResponse {
    if !is_safe_param(&key) || !is_safe_param(&id) || !is_safe_param(&size_str) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let client = ImmichClient::new();
    let cookie_password = get_cookie_password(&headers, &key);

    let mut params = vec![("key", key.as_str())];
    if let Some(ref pwd) = cookie_password {
        params.push(("password", pwd.as_str()));
    }

    let subpath = if size_str == "original" {
        format!("/assets/{}/original", id)
    } else {
        params.push(("size", size_str.as_str()));
        format!("/assets/{}/thumbnail", id)
    };

    let url = client.build_url(&subpath, &params);

    let mut req = client.http_client.get(&url);
    if let Some(range) = headers.get(axum::http::header::RANGE) {
        req = req.header(reqwest::header::RANGE, range.clone());
    }

    let res = match req.send().await {
        Ok(res) => res,
        Err(e) => {
            eprintln!(
                "proxy_photo: upstream request failed for asset {}: {}",
                id, e
            );
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let mut builder = axum::response::Response::builder().status(res.status());
    for (k, v) in res.headers() {
        if [
            "content-type",
            "content-length",
            "etag",
            "last-modified",
            "content-disposition",
            "cache-control",
            "accept-ranges",
            "content-range",
        ]
        .contains(&k.as_str())
        {
            builder = builder.header(k.clone(), v.clone());
        }
    }
    builder.body(Body::from_stream(res.bytes_stream())).unwrap()
}

pub async fn proxy_video(
    headers: HeaderMap,
    Path((key, id)): Path<(String, String)>,
) -> impl IntoResponse {
    if !is_safe_param(&key) || !is_safe_param(&id) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let client = ImmichClient::new();
    let cookie_password = get_cookie_password(&headers, &key);

    let mut params = vec![("key", key.as_str())];
    if let Some(ref pwd) = cookie_password {
        params.push(("password", pwd.as_str()));
    }

    let url = client.build_url(&format!("/assets/{}/video/playback", id), &params);

    let mut req = client.http_client.get(&url);
    if let Some(range) = headers.get(axum::http::header::RANGE) {
        req = req.header(reqwest::header::RANGE, range.clone());
    }

    let res = match req.send().await {
        Ok(res) => res,
        Err(e) => {
            eprintln!(
                "proxy_video: upstream request failed for asset {}: {}",
                id, e
            );
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let mut builder = axum::response::Response::builder().status(res.status());
    for (k, v) in res.headers() {
        if [
            "content-type",
            "content-length",
            "etag",
            "last-modified",
            "content-range",
            "accept-ranges",
            "cache-control",
        ]
        .contains(&k.as_str())
        {
            builder = builder.header(k.clone(), v.clone());
        }
    }
    builder.body(Body::from_stream(res.bytes_stream())).unwrap()
}

#[derive(Deserialize)]
pub struct DownloadQuery {
    pub asset_ids: Option<String>,
}

pub async fn download_all(
    headers: HeaderMap,
    Path(key): Path<String>,
    Query(query): Query<DownloadQuery>,
) -> impl IntoResponse {
    if !is_safe_param(&key) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let client = ImmichClient::new();
    let cookie_password = get_cookie_password(&headers, &key);

    let mut params = vec![("key", key.as_str())];
    if let Some(ref pwd) = cookie_password {
        params.push(("password", pwd.as_str()));
    }

    let url = client.build_url("/shared-links/me", &params);
    let res = client.http_client.get(&url).send().await;
    let mut share: crate::immich_client::model::SharedLink = match res {
        Ok(r) if r.status().is_success() => match r.json().await {
            Ok(data) => data,
            Err(e) => {
                eprintln!(
                    "download_all: failed to parse share link response for key '{}': {}",
                    key, e
                );
                return IntoResponse::into_response(StatusCode::INTERNAL_SERVER_ERROR);
            }
        },
        Ok(r) => {
            eprintln!(
                "download_all: share link request failed for key '{}': {}",
                key,
                r.status()
            );
            return IntoResponse::into_response(StatusCode::UNAUTHORIZED);
        }
        Err(e) => {
            eprintln!(
                "download_all: upstream request failed for key '{}': {}",
                key, e
            );
            return IntoResponse::into_response(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    if share.r#type.as_deref() == Some("ALBUM") {
        if let Some(ref album) = share.album {
            let album_url = client.build_url(&format!("/albums/{}", album.id), &params);
            if let Ok(album_res) = client.http_client.get(&album_url).send().await {
                if let Ok(album_data) = album_res.json::<crate::immich_client::model::Album>().await
                {
                    share.assets = album_data.assets;
                }
            }
        }
    }

    let title = share
        .description
        .clone()
        .or_else(|| share.album.as_ref().and_then(|a| a.album_name.clone()))
        .unwrap_or_else(|| {
            if share.r#type.as_deref() == Some("INDIVIDUAL") {
                share
                    .assets
                    .first()
                    .and_then(|a| a.original_file_name.clone())
                    .unwrap_or_else(|| "shared_assets".to_string())
            } else {
                "shared_assets".to_string()
            }
        });
    let sanitized_title = title.replace(|c: char| !c.is_alphanumeric() && c != '-', "_");
    let filename = format!("{}.zip", sanitized_title);

    let asset_ids: Vec<String> = if let Some(ids_str) = query.asset_ids {
        ids_str
            .split(',')
            .filter(|s| is_safe_param(s))
            .map(|s| s.to_string())
            .collect()
    } else {
        share.assets.into_iter().map(|a| a.id).collect()
    };

    if asset_ids.is_empty() {
        return IntoResponse::into_response(StatusCode::NOT_FOUND);
    }

    let payload = serde_json::json!({
        "assetIds": asset_ids
    });

    let download_url = client.build_url("/download/archive", &params);
    let res = match client
        .http_client
        .post(&download_url)
        .json(&payload)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            eprintln!(
                "download_all: archive download failed for key '{}': {}",
                key,
                r.status()
            );
            return IntoResponse::into_response(StatusCode::INTERNAL_SERVER_ERROR);
        }
        Err(e) => {
            eprintln!(
                "download_all: upstream archive request failed for key '{}': {}",
                key, e
            );
            return IntoResponse::into_response(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        "application/zip".parse().unwrap(),
    );
    let ascii_filename: String = filename
        .chars()
        .map(|c| if c.is_ascii_graphic() || c == ' ' { c } else { '_' })
        .collect();
    let encoded_filename = urlencoding::encode(&filename);
    headers.insert(
        axum::http::header::CONTENT_DISPOSITION,
        format!(
            "attachment; filename=\"{}\"; filename*=UTF-8''{}",
            ascii_filename, encoded_filename
        )
        .parse()
        .unwrap(),
    );

    (headers, Body::from_stream(res.bytes_stream())).into_response()
}

static ADDED_ALBUMS: std::sync::OnceLock<parking_lot::RwLock<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();
static TAG_CACHE: std::sync::OnceLock<
    parking_lot::RwLock<std::collections::HashMap<String, String>>,
> = std::sync::OnceLock::new();
static PROCESSED_ASSETS: std::sync::OnceLock<
    parking_lot::RwLock<std::collections::HashMap<String, (String, std::time::Instant)>>,
> = std::sync::OnceLock::new();

async fn get_or_create_tag(
    client: &ImmichClient,
    name: &str,
    parent_id: Option<&str>,
) -> Option<String> {
    let cache =
        TAG_CACHE.get_or_init(|| parking_lot::RwLock::new(std::collections::HashMap::new()));

    let cache_key = match parent_id {
        Some(p_id) => format!("{}:{}", p_id, name),
        None => format!("root:{}", name),
    };

    {
        let read_guard = cache.read();
        if let Some(id) = read_guard.get(&cache_key) {
            return Some(id.clone());
        }
    }

    let upload_key = client.upload_api_key.as_ref()?;

    // Step 1: List all tags and bulk-cache them
    let get_url = client.build_url("/tags", &[]);
    let res = client
        .http_client
        .get(&get_url)
        .header("x-api-key", upload_key)
        .send()
        .await
        .ok()?;

    if res.status().is_success() {
        if let Ok(tags) = res.json::<Vec<crate::immich_client::model::Tag>>().await {
            let mut write_guard = cache.write();
            for tag in &tags {
                let key = match &tag.parent_id {
                    Some(p_id) => format!("{}:{}", p_id, tag.name),
                    None => format!("root:{}", tag.name),
                };
                write_guard.entry(key).or_insert_with(|| tag.id.clone());
            }
            if let Some(id) = write_guard.get(&cache_key) {
                return Some(id.clone());
            }
        }
    }

    // Step 2: Create tag if not found
    let post_url = client.build_url("/tags", &[]);
    let create_body = serde_json::json!({
        "name": name,
        "parentId": parent_id,
    });

    let create_res = client
        .http_client
        .post(&post_url)
        .header("x-api-key", upload_key)
        .json(&create_body)
        .send()
        .await
        .ok()?;

    let status = create_res.status();
    if status.is_success() || status == StatusCode::CREATED {
        if let Ok(created_tag) = create_res.json::<crate::immich_client::model::Tag>().await {
            let mut write_guard = cache.write();
            write_guard.insert(cache_key, created_tag.id.clone());
            return Some(created_tag.id);
        }
    } else {
        // Tag might have been created concurrently by another thread.
        // Query /tags again to find the concurrently created tag.
        let get_url = client.build_url("/tags", &[]);
        let retry_res = client
            .http_client
            .get(&get_url)
            .header("x-api-key", upload_key)
            .send()
            .await
            .ok()?;

        if retry_res.status().is_success() {
            if let Ok(tags) = retry_res
                .json::<Vec<crate::immich_client::model::Tag>>()
                .await
            {
                let mut write_guard = cache.write();
                for tag in &tags {
                    let key = match &tag.parent_id {
                        Some(p_id) => format!("{}:{}", p_id, tag.name),
                        None => format!("root:{}", tag.name),
                    };
                    write_guard.insert(key, tag.id.clone());
                }
                if let Some(id) = write_guard.get(&cache_key) {
                    return Some(id.clone());
                }
            }
        }
    }

    None
}

static IMMICH_API_SEMAPHORE: std::sync::OnceLock<tokio::sync::Semaphore> =
    std::sync::OnceLock::new();

async fn tag_and_associate_asset(
    client: &ImmichClient,
    asset_id: &str,
    album_id: &str,
    uploader_name: &str,
) -> bool {
    let sem = IMMICH_API_SEMAPHORE.get_or_init(|| tokio::sync::Semaphore::new(1));
    let _permit = sem.acquire().await.ok();

    let mut trash_checked = false;
    let mut tagged = false;
    let mut added_to_album = false;

    // Retry loop: we will try up to 10 times to perform the remaining steps
    for attempt in 1..=10 {
        // Step 1: Check trash status and restore if needed
        if !trash_checked {
            let get_asset_url = client.build_url(&format!("/assets/{}", asset_id), &[]);
            let asset_res = client
                .http_client
                .get(&get_asset_url)
                .header("x-api-key", client.upload_api_key.as_ref().unwrap())
                .send()
                .await;

            match asset_res {
                Ok(r) if r.status().is_success() => {
                    if let Ok(asset_info) = r.json::<crate::immich_client::model::Asset>().await {
                        if asset_info.is_trashed.unwrap_or(false) {
                            println!(
                                "upload: asset '{}' is in trash, attempting to restore (attempt {})...",
                                asset_id, attempt
                            );
                            let restore_url = client.build_url("/trash/restore/assets", &[]);
                            let restore_body = serde_json::json!({ "ids": [asset_id] });
                            let restore_res = client
                                .http_client
                                .post(&restore_url)
                                .header("x-api-key", client.upload_api_key.as_ref().unwrap())
                                .json(&restore_body)
                                .send()
                                .await;

                            match restore_res {
                                Ok(res) if res.status().is_success() => {
                                    println!(
                                        "upload: successfully restored asset '{}' from trash",
                                        asset_id
                                    );
                                    trash_checked = true;
                                }
                                Ok(res) => {
                                    eprintln!(
                                        "upload: failed to restore asset '{}' from trash: status {} (attempt {})",
                                        asset_id,
                                        res.status(),
                                        attempt
                                    );
                                }
                                Err(e) => {
                                    eprintln!(
                                        "upload: failed to send restore request for asset '{}': {} (attempt {})",
                                        asset_id, e, attempt
                                    );
                                }
                            }
                        } else {
                            // Asset exists and is not in trash
                            trash_checked = true;
                        }
                    } else {
                        eprintln!(
                            "upload: failed to parse asset response for '{}' (attempt {})",
                            asset_id, attempt
                        );
                    }
                }
                Ok(r) => {
                    eprintln!(
                        "upload: checking asset '{}' returned status {} (attempt {})",
                        asset_id,
                        r.status(),
                        attempt
                    );
                }
                Err(e) => {
                    eprintln!(
                        "upload: failed to check asset '{}': {} (attempt {})",
                        asset_id, e, attempt
                    );
                }
            }
        }

        // Step 2: Tag the asset with uploader name
        if trash_checked && !tagged {
            if let Some(parent_tag_id) = get_or_create_tag(client, "SharedBy", None).await {
                if let Some(child_tag_id) =
                    get_or_create_tag(client, uploader_name, Some(&parent_tag_id)).await
                {
                    let tag_url = client.build_url(&format!("/tags/{}/assets", child_tag_id), &[]);
                    let tag_res = client
                        .http_client
                        .put(&tag_url)
                        .header("x-api-key", client.upload_api_key.as_ref().unwrap())
                        .json(&serde_json::json!({ "ids": [asset_id] }))
                        .send()
                        .await;

                    #[derive(serde::Deserialize)]
                    struct TagResponse {
                        #[allow(dead_code)]
                        id: String,
                        success: bool,
                    }

                    match tag_res {
                        Ok(res) if res.status().is_success() => {
                            if let Ok(results) = res.json::<Vec<TagResponse>>().await {
                                if let Some(first) = results.first() {
                                    if first.success {
                                        // Verify that the tag was actually applied, checking a few times if needed
                                        let mut actual_tagged = false;
                                        for check_attempt in 1..=4 {
                                            if check_attempt > 1 {
                                                tokio::time::sleep(
                                                    std::time::Duration::from_millis(500),
                                                )
                                                .await;
                                            }

                                            let verify_asset_url = client
                                                .build_url(&format!("/assets/{}", asset_id), &[]);
                                            let verify_res = client
                                                .http_client
                                                .get(&verify_asset_url)
                                                .header(
                                                    "x-api-key",
                                                    client.upload_api_key.as_ref().unwrap(),
                                                )
                                                .send()
                                                .await;

                                            if let Ok(r) = verify_res {
                                                if r.status().is_success() {
                                                    if let Ok(asset_info) = r
                                                        .json::<crate::immich_client::model::Asset>(
                                                        )
                                                        .await
                                                    {
                                                        if let Some(ref asset_tags) =
                                                            asset_info.tags
                                                        {
                                                            if asset_tags
                                                                .iter()
                                                                .any(|t| t.id == child_tag_id)
                                                            {
                                                                actual_tagged = true;
                                                                break;
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }

                                        if actual_tagged {
                                            tagged = true;
                                        } else {
                                            eprintln!(
                                                "upload: tagging returned success but tag {} was not found on asset {} after verification (attempt {})",
                                                child_tag_id, asset_id, attempt
                                            );
                                        }
                                    } else {
                                        eprintln!(
                                            "upload: tagging returned success:false for asset {} (attempt {})",
                                            asset_id, attempt
                                        );
                                    }
                                } else {
                                    eprintln!(
                                        "upload: tagging returned empty list for asset {} (attempt {})",
                                        asset_id, attempt
                                    );
                                }
                            } else {
                                eprintln!(
                                    "upload: failed to parse tag response for asset {} (attempt {})",
                                    asset_id, attempt
                                );
                            }
                        }
                        Ok(res) => {
                            eprintln!(
                                "upload: tagging failed for asset {} with status {} (attempt {})",
                                asset_id,
                                res.status(),
                                attempt
                            );
                        }
                        Err(e) => {
                            eprintln!(
                                "upload: tagging request failed for asset {}: {} (attempt {})",
                                asset_id, e, attempt
                            );
                        }
                    }
                } else {
                    eprintln!(
                        "upload: failed to get or create child tag '{}' (attempt {})",
                        uploader_name, attempt
                    );
                }
            } else {
                eprintln!(
                    "upload: failed to get or create parent tag 'SharedBy' (attempt {})",
                    attempt
                );
            }
        }

        // Step 3: Add to album
        if trash_checked && !added_to_album {
            let album_url = client.build_url(&format!("/albums/{}/assets", album_id), &[]);
            let album_res = client
                .http_client
                .put(&album_url)
                .header("x-api-key", client.upload_api_key.as_ref().unwrap())
                .json(&serde_json::json!({ "ids": [asset_id] }))
                .send()
                .await;

            match album_res {
                Ok(res) if res.status().is_success() => {
                    added_to_album = true;
                }
                Ok(res) => {
                    eprintln!(
                        "upload: failed to add asset {} to album {}: status {} (attempt {})",
                        asset_id,
                        album_id,
                        res.status(),
                        attempt
                    );
                }
                Err(e) => {
                    eprintln!(
                        "upload: failed to send add-to-album request for asset {} to album {}: {} (attempt {})",
                        asset_id, album_id, e, attempt
                    );
                }
            }
        }

        // If all operations succeeded, we can stop!
        if trash_checked && tagged && added_to_album {
            break;
        }

        // Wait before the next attempt, with exponential backoff.
        let delay_ms = (250 * attempt as u64).min(2000);
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
    }

    if !trash_checked || !tagged || !added_to_album {
        eprintln!(
            "upload: finished processing asset {} with status: trash_checked={}, tagged={}, added_to_album={}",
            asset_id, trash_checked, tagged, added_to_album
        );
    }

    trash_checked && tagged && added_to_album
}

pub async fn upload_asset_handler(
    headers: HeaderMap,
    Path(key): Path<String>,
    req: Request,
) -> impl IntoResponse {
    if !is_safe_param(&key) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    if !check_csrf(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }

    let client = ImmichClient::new();

    // If service account key is not set, disable upload functionality entirely.
    if client.upload_api_key.is_none() {
        return StatusCode::FORBIDDEN.into_response();
    }

    // Parse uploader name header
    let uploader_name = match headers.get("x-uploader-name").and_then(|h| h.to_str().ok()) {
        Some(val) if !val.is_empty() => match urlencoding::decode(val) {
            Ok(decoded) => decoded.into_owned(),
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        },
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let cookie_password = get_cookie_password(&headers, &key);

    // Validate share key first
    let share_link = match client
        .fetch_share_me(&key, cookie_password.as_deref())
        .await
    {
        Ok((status, text)) if status.is_success() => {
            match serde_json::from_str::<crate::immich_client::model::SharedLink>(&text) {
                Ok(link) => link,
                Err(e) => {
                    eprintln!("upload: failed to parse share link response: {}", e);
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
        Ok((status, _)) => return status.into_response(),
        Err(e) => {
            eprintln!("upload: failed to fetch share link: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if share_link.r#type.as_deref() != Some("ALBUM") || !share_link.allow_upload.unwrap_or(false) {
        return StatusCode::FORBIDDEN.into_response();
    }

    let album_id = match share_link.album.as_ref() {
        Some(album) => &album.id,
        None => return StatusCode::BAD_REQUEST.into_response(),
    };

    // Ensure the service account user is a contributor (editor) to the album
    let service_account_user_id = match client.get_upload_user_id().await {
        Some(id) => id,
        None => {
            eprintln!("upload: failed to resolve upload user ID");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let is_added = {
        let cache =
            ADDED_ALBUMS.get_or_init(|| parking_lot::RwLock::new(std::collections::HashSet::new()));
        let read_guard = cache.read();
        read_guard.contains(album_id)
    };

    if !is_added {
        let add_users_body = serde_json::json!({
            "albumUsers": [
                {
                    "userId": service_account_user_id,
                    "role": "editor"
                }
            ]
        });
        let add_res = client
            .admin_put(&format!("/albums/{}/users", album_id), &add_users_body)
            .await;
        let mut add_success = false;
        if let Some(res) = add_res {
            let status = res.status();
            if status.is_success() || status == StatusCode::CONFLICT {
                add_success = true;
            } else if status == StatusCode::BAD_REQUEST {
                let body = res.text().await.unwrap_or_default();
                if body.contains("already") {
                    add_success = true;
                } else {
                    eprintln!(
                        "upload: failed to add service account to album {}: status {} — {}",
                        album_id, status, body
                    );
                }
            } else {
                eprintln!(
                    "upload: failed to add service account to album {}: status {}",
                    album_id, status
                );
            }
        } else {
            eprintln!("upload: failed to send add user request (admin key missing)");
        }

        if add_success {
            let cache = ADDED_ALBUMS.get().unwrap();
            let mut write_guard = cache.write();
            write_guard.insert(album_id.clone());
        }
    }

    // Extract crucial headers from the incoming request.
    let content_type = headers.get(axum::http::header::CONTENT_TYPE).cloned();
    let content_length = headers.get(axum::http::header::CONTENT_LENGTH).cloned();

    // Stream the raw axum body directly into the reqwest body.
    let stream = req.into_body().into_data_stream();
    let reqwest_body = reqwest::Body::wrap_stream(stream);

    // Forward the streamed request to Immich using the service account API key
    let url = client.build_url("/assets", &[]);
    let mut out_req = client
        .http_client
        .post(&url)
        .header("x-api-key", client.upload_api_key.as_ref().unwrap())
        .body(reqwest_body);

    if let Some(ct) = content_type {
        out_req = out_req.header(reqwest::header::CONTENT_TYPE, ct);
    }
    if let Some(cl) = content_length {
        out_req = out_req.header(reqwest::header::CONTENT_LENGTH, cl);
    }

    let res = match out_req.send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            eprintln!(
                "upload: upstream rejected asset for key '{}': {} — {}",
                key, status, body
            );
            return status.into_response();
        }
        Err(e) => {
            eprintln!("upload: upstream request failed for key '{}': {}", key, e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Parse the uploaded asset ID from the response
    #[derive(Deserialize)]
    struct AssetUploadResponse {
        id: String,
    }

    let upload_resp: AssetUploadResponse = match res.json().await {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "upload: failed to parse upload response for key '{}': {}",
                key, e
            );
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let asset_id = upload_resp.id;
    // Spawn background task to tag and associate the asset, saving it to PROCESSED_ASSETS when done.
    let client_clone = client.clone();
    let asset_id_clone = asset_id.clone();
    let album_id_clone = album_id.clone();
    let uploader_name_clone = uploader_name.clone();

    tokio::spawn(async move {
        let success = tag_and_associate_asset(
            &client_clone,
            &asset_id_clone,
            &album_id_clone,
            &uploader_name_clone,
        )
        .await;

        if success {
            let cache = PROCESSED_ASSETS
                .get_or_init(|| parking_lot::RwLock::new(std::collections::HashMap::new()));
            let mut write_guard = cache.write();

            // Clean up stale entries to prevent memory leaks from abandoned status polls
            let now = std::time::Instant::now();
            let expiry = std::time::Duration::from_secs(600); // 10 minutes
            write_guard.retain(|_, (_, timestamp)| now.duration_since(*timestamp) < expiry);

            write_guard.insert(asset_id_clone, (uploader_name_clone, now));
        }
    });

    #[derive(serde::Serialize)]
    struct UploadSuccessResponse {
        id: String,
    }

    (
        StatusCode::OK,
        axum::Json(UploadSuccessResponse { id: asset_id }),
    )
        .into_response()
}

pub async fn upload_status_handler(
    headers: HeaderMap,
    Path((key, asset_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if !is_safe_param(&key) || !is_safe_param(&asset_id) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let client = ImmichClient::new();
    if client.upload_api_key.is_none() {
        return StatusCode::FORBIDDEN.into_response();
    }

    let cookie_password = get_cookie_password(&headers, &key);

    // Validate share key first
    let share_link = match client
        .fetch_share_me(&key, cookie_password.as_deref())
        .await
    {
        Ok((status, text)) if status.is_success() => {
            match serde_json::from_str::<crate::immich_client::model::SharedLink>(&text) {
                Ok(link) => link,
                Err(e) => {
                    eprintln!("status: failed to parse share link response: {}", e);
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
        Ok((status, _)) => return status.into_response(),
        Err(e) => {
            eprintln!("status: failed to fetch share link: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if share_link.r#type.as_deref() != Some("ALBUM") || !share_link.allow_upload.unwrap_or(false) {
        return StatusCode::FORBIDDEN.into_response();
    }

    // Check if the asset is in the PROCESSED_ASSETS map
    let uploader_name = {
        let cache = PROCESSED_ASSETS
            .get_or_init(|| parking_lot::RwLock::new(std::collections::HashMap::new()));
        let read_guard = cache.read();
        match read_guard.get(&asset_id) {
            Some((name, _)) => name.clone(),
            None => return StatusCode::ACCEPTED.into_response(), // 202 Accepted: still processing tagging / album association
        }
    };

    // Check if the thumbnail has been generated by Immich
    let get_thumb_url = client.build_url(
        &format!("/assets/{}/thumbnail", asset_id),
        &[("size", "thumbnail")],
    );
    let thumb_res = client
        .http_client
        .head(&get_thumb_url)
        .header("x-api-key", client.upload_api_key.as_ref().unwrap())
        .send()
        .await;

    match thumb_res {
        Ok(r) if r.status().is_success() => {}
        _ => return StatusCode::ACCEPTED.into_response(), // 202 Accepted: thumbnail not ready yet
    }

    // Fetch the final asset info from Immich to construct a complete SafeAsset
    let get_asset_url = client.build_url(&format!("/assets/{}", asset_id), &[]);
    let asset_res = client
        .http_client
        .get(&get_asset_url)
        .header("x-api-key", client.upload_api_key.as_ref().unwrap())
        .send()
        .await;

    let asset = match asset_res {
        Ok(r) if r.status().is_success() => {
            match r.json::<crate::immich_client::model::Asset>().await {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("status: failed to parse asset response: {}", e);
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            }
        }
        Ok(r) => {
            eprintln!("status: fetch asset returned status {}", r.status());
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        Err(e) => {
            eprintln!("status: fetch asset request failed: {}", e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let mut safe_asset = crate::dto::SafeAsset::from_base(asset);
    safe_asset.uploader_name = Some(uploader_name);
    safe_asset.uploader_is_fallback = false;

    if share_link.allow_download.unwrap_or(false) {
        safe_asset.download_url = Some(format!("/share/photo/{}/{}/original", key, safe_asset.id));
    }

    // Successfully processed, clean up the cache
    {
        let cache = PROCESSED_ASSETS
            .get_or_init(|| parking_lot::RwLock::new(std::collections::HashMap::new()));
        let mut write_guard = cache.write();
        write_guard.remove(&asset_id);
    }

    (StatusCode::OK, axum::Json(safe_asset)).into_response()
}
