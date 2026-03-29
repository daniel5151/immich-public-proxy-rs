use crate::immich_client::client::ImmichClient;
use crate::immich_client::client::get_cookie_password;
use axum::body::Body;
use axum::extract::Form;
use axum::extract::Path;
use axum::extract::Query;
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

pub async fn unlock_share_handler(
    headers: HeaderMap,
    Form(payload): Form<UnlockPayload>,
) -> impl IntoResponse {
    if !is_safe_param(&payload.key) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    if let Some(site) = headers.get("sec-fetch-site") {
        if site != "same-origin" {
            return StatusCode::FORBIDDEN.into_response();
        }
    } else if let (Some(o), Some(h)) = (
        headers
            .get(axum::http::header::ORIGIN)
            .and_then(|v| v.to_str().ok()),
        headers
            .get(axum::http::header::HOST)
            .and_then(|v| v.to_str().ok()),
    ) {
        if !o.ends_with(&format!("://{}", h)) {
            return StatusCode::FORBIDDEN.into_response();
        }
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

        let cookie1 = format!(
            "immich_pwd_{}={}; Path=/; HttpOnly; Secure; SameSite=Lax",
            b64_key, b64_pwd
        );
        let mut resp = Redirect::to(&format!("/share/{}", payload.key)).into_response();
        resp.headers_mut()
            .append(axum::http::header::SET_COOKIE, cookie1.parse().unwrap());

        if payload.key != real_key {
            let b64_real_key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&real_key);
            let cookie2 = format!(
                "immich_pwd_{}={}; Path=/; HttpOnly; Secure; SameSite=Lax",
                b64_real_key, b64_pwd
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
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
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
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
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
            Err(_) => return IntoResponse::into_response(StatusCode::INTERNAL_SERVER_ERROR),
        },
        _ => {
            return IntoResponse::into_response(StatusCode::UNAUTHORIZED);
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
        .expect("share link missing title/description for download");
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
        _ => return IntoResponse::into_response(StatusCode::INTERNAL_SERVER_ERROR),
    };

    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        "application/zip".parse().unwrap(),
    );
    let encoded_filename = urlencoding::encode(&filename);
    headers.insert(
        axum::http::header::CONTENT_DISPOSITION,
        format!("attachment; filename*=UTF-8''{}", encoded_filename)
            .parse()
            .unwrap(),
    );

    (headers, Body::from_stream(res.bytes_stream())).into_response()
}

fn get_upload_api_key() -> Option<&'static str> {
    static KEY: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    KEY.get_or_init(|| std::env::var("IMMICH_API_KEY_UPLOAD_USER").ok())
        .as_deref()
}

fn get_admin_album_api_key() -> Option<&'static str> {
    static KEY: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    KEY.get_or_init(|| std::env::var("IMMICH_API_KEY_ADMIN_ALBUM_USER_ADD").ok())
        .as_deref()
}

/// Resolve the upload user's ID by calling GET /users/me with their API key.
/// Cached after the first successful call.
async fn get_upload_user_id(client: &ImmichClient, api_key: &str) -> Option<String> {
    use tokio::sync::OnceCell;
    static USER_ID: OnceCell<Option<String>> = OnceCell::const_new();
    USER_ID
        .get_or_init(|| async {
            #[derive(Deserialize)]
            struct UserMe {
                id: String,
            }
            let url = client.build_url("/users/me", &[]);
            let res = client
                .http_client
                .get(&url)
                .header("x-api-key", api_key)
                .send()
                .await
                .ok()?;
            if !res.status().is_success() {
                eprintln!("failed to resolve upload user: HTTP {}", res.status());
                return None;
            }
            let user: UserMe = res.json().await.ok()?;
            Some(user.id)
        })
        .await
        .clone()
}

/// Ensure the upload user is a collaborator on the given album.
/// Uses the admin API key (which has albumUser.create permission).
/// Results are cached per album_id for the lifetime of the server.
async fn ensure_album_collaborator(
    client: &ImmichClient,
    album_id: &str,
    upload_user_id: &str,
    admin_api_key: &str,
    upload_api_key: &str,
) {
    let check_url = client.build_url(
        &format!("/albums/{}", album_id),
        &[("withoutAssets", "true")],
    );

    // 1. Check if the upload user is already a collaborator
    if let Ok(res) = client
        .http_client
        .get(&check_url)
        .header("x-api-key", upload_api_key)
        .send()
        .await
    {
        if res.status().is_success() {
            return;
        }
    }

    // 2. Add the user as a collaborator
    let url = client.build_url(&format!("/albums/{}/users", album_id), &[]);
    let body = serde_json::json!({
        "albumUsers": [{ "userId": upload_user_id, "role": "editor" }]
    });
    let res = client
        .http_client
        .put(&url)
        .header("x-api-key", admin_api_key)
        .json(&body)
        .send()
        .await;

    match res {
        Ok(r) if r.status().is_success() || r.status() == 400 => {
            // 3. Validation loop: wait until Immich has granted access
            for _ in 0..10 {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if let Ok(check_res) = client
                    .http_client
                    .get(&check_url)
                    .header("x-api-key", upload_api_key)
                    .send()
                    .await
                {
                    if check_res.status().is_success() {
                        return;
                    }
                }
            }
            eprintln!(
                "WARNING: Timed out waiting for upload user to be granted access to album {}",
                album_id
            );
        }
        Ok(r) => {
            eprintln!(
                "failed to add upload user to album {}: HTTP {}",
                album_id,
                r.status()
            );
        }
        Err(e) => {
            eprintln!("failed to add upload user to album {}: {}", album_id, e);
        }
    }
}

#[derive(Deserialize)]
pub struct UploadQuery {
    pub uploader_name: Option<String>,
}

pub async fn upload_asset_handler(
    headers: HeaderMap,
    Path(key): Path<String>,
    Query(upload_query): Query<UploadQuery>,
    mut multipart: axum::extract::Multipart,
) -> impl IntoResponse {
    if !is_safe_param(&key) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let uploader_name = match upload_query.uploader_name {
        Some(ref name) if !name.trim().is_empty() => name.trim().to_string(),
        _ => return (StatusCode::BAD_REQUEST, "uploader_name is required").into_response(),
    };

    let upload_api_key = match get_upload_api_key() {
        Some(k) => k,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Upload not configured (missing IMMICH_API_KEY_UPLOAD_USER)",
            )
                .into_response();
        }
    };

    let admin_api_key = match get_admin_album_api_key() {
        Some(k) => k,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Upload not configured (missing IMMICH_API_KEY_ADMIN_ALBUM_USER_ADD)",
            )
                .into_response();
        }
    };

    // CSRF check (same as unlock_share_handler)
    if let Some(site) = headers.get("sec-fetch-site") {
        if site != "same-origin" {
            return StatusCode::FORBIDDEN.into_response();
        }
    } else if let (Some(o), Some(h)) = (
        headers
            .get(axum::http::header::ORIGIN)
            .and_then(|v| v.to_str().ok()),
        headers
            .get(axum::http::header::HOST)
            .and_then(|v| v.to_str().ok()),
    ) {
        if !o.ends_with(&format!("://{}", h)) {
            return StatusCode::FORBIDDEN.into_response();
        }
    }

    let client = ImmichClient::new();
    let cookie_password = get_cookie_password(&headers, &key);

    // Read the entire multipart body FIRST before doing any async network calls.
    // Multipart is a lazy stream over the request body, so we must drain it before
    // making outbound HTTP requests, otherwise the stream can stall/corrupt.
    let mut request_form = reqwest::multipart::Form::new();
    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();

        if name == "assetData" {
            let file_name = field.file_name().unwrap_or("unknown").to_string();
            let content_type = field
                .content_type()
                .unwrap_or("application/octet-stream")
                .to_string();
            let data = match field.bytes().await {
                Ok(b) => b,
                Err(_) => return StatusCode::BAD_REQUEST.into_response(),
            };

            let part = reqwest::multipart::Part::bytes(data.to_vec())
                .file_name(file_name)
                .mime_str(&content_type)
                .unwrap_or_else(|_| {
                    reqwest::multipart::Part::bytes(vec![]) // unreachable in practice
                });

            request_form = request_form.part(name, part);
        } else {
            let text = field.text().await.unwrap_or_default();
            request_form = request_form.text(name, text);
        }
    }

    // Resolve album ID from shared link
    let share_link: Option<crate::immich_client::model::SharedLink> = client
        .fetch_share_me(&key, cookie_password.as_deref())
        .await
        .ok()
        .and_then(|(status, text)| {
            if status.is_success() {
                serde_json::from_str(&text).ok()
            } else {
                None
            }
        });

    let album_id = match share_link
        .as_ref()
        .and_then(|link| link.album.as_ref())
        .map(|album| album.id.clone())
    {
        Some(id) => id,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                "Upload is only supported for album shares",
            )
                .into_response();
        }
    };

    // Resolve the upload user's ID
    let upload_user_id = match get_upload_user_id(&client, upload_api_key).await {
        Some(id) => id,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to resolve upload user",
            )
                .into_response();
        }
    };

    // Ensure the upload user is a collaborator on the album
    ensure_album_collaborator(
        &client,
        &album_id,
        &upload_user_id,
        admin_api_key,
        upload_api_key,
    )
    .await;

    // Upload the asset using the upload user's API key
    let url = client.build_url("/assets", &[]);
    let res = match client
        .http_client
        .post(&url)
        .header("x-api-key", upload_api_key)
        .multipart(request_form)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            eprintln!("upload failed: HTTP {} - {}", status, body);
            return status.into_response();
        }
        Err(e) => {
            eprintln!("upload request failed: {}", e);
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
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    let asset_id = upload_resp.id;

    // Wait for Immich to finish processing the asset before applying metadata.
    // Poll GET /assets/{id} until `hasMetadata` is true (or up to ~7.5 seconds).
    {
        #[derive(Deserialize)]
        struct AssetCheck {
            #[serde(default)]
            #[serde(rename = "hasMetadata")]
            has_metadata: bool,
        }

        let check_url = client.build_url(&format!("/assets/{}", asset_id), &[]);
        for attempt in 0..15 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if let Ok(r) = client
                .http_client
                .get(&check_url)
                .header("x-api-key", upload_api_key)
                .send()
                .await
            {
                if r.status().is_success() {
                    if let Ok(check) = r.json::<AssetCheck>().await {
                        if check.has_metadata {
                            eprintln!(
                                "[{}] metadata ready after {}ms",
                                asset_id,
                                (attempt + 1) * 500
                            );
                            break;
                        }
                    }
                }
            }
            if attempt == 14 {
                eprintln!(
                    "[{}] WARNING: still no metadata after 7.5s; proceeding anyway",
                    asset_id
                );
            }
        }
    }

    // Helper: retry a request-building closure up to `max_retries` times with backoff.
    // Returns true if the request eventually succeeded.
    // `accept_400`: if true, treat HTTP 400 as success (for idempotent ops like album add).
    async fn retry_request<F, Fut>(
        asset_id: &str,
        label: &str,
        max_retries: u32,
        accept_400: bool,
        f: F,
    ) -> bool
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<reqwest::Response, reqwest::Error>>,
    {
        for attempt in 0..=max_retries {
            match f().await {
                Ok(r) if r.status().is_success() => return true,
                Ok(r) if accept_400 && r.status() == 400 => return true,
                Ok(r) => {
                    let status = r.status();
                    let body = r.text().await.unwrap_or_default();
                    eprintln!(
                        "[{}] {} attempt {}/{}: HTTP {} - {}",
                        asset_id,
                        label,
                        attempt + 1,
                        max_retries + 1,
                        status,
                        body
                    );
                }
                Err(e) => {
                    eprintln!(
                        "[{}] {} attempt {}/{}: {}",
                        asset_id,
                        label,
                        attempt + 1,
                        max_retries + 1,
                        e
                    );
                }
            }
            if attempt < max_retries {
                tokio::time::sleep(std::time::Duration::from_millis(500 * 2u64.pow(attempt))).await;
            }
        }
        eprintln!(
            "[{}] {}: all {} retries exhausted",
            asset_id,
            label,
            max_retries + 1
        );
        false
    }

    let mut failures: Vec<&str> = Vec::new();

    // Set the asset description
    {
        let desc = format!("Uploaded by: {}", uploader_name);
        let asset_url = client.build_url(&format!("/assets/{}", asset_id), &[]);
        let ok = retry_request(&asset_id, "set-description", 2, false, || {
            client
                .http_client
                .put(&asset_url)
                .header("x-api-key", upload_api_key)
                .json(&serde_json::json!({ "description": desc }))
                .send()
        })
        .await;
        if !ok {
            failures.push("set-description");
        }
    }

    // Add the uploaded asset to the album (accept 400 = already in album)
    {
        let album_assets_url = client.build_url(&format!("/albums/{}/assets", album_id), &[]);
        let ok = retry_request(&asset_id, "add-to-album", 2, true, || {
            client
                .http_client
                .put(&album_assets_url)
                .header("x-api-key", upload_api_key)
                .json(&serde_json::json!({ "ids": [asset_id] }))
                .send()
        })
        .await;
        if !ok {
            failures.push("add-to-album");
        }
    }

    // Tag the asset with "uploaded-by: {name}"
    {
        let tag_name = format!("uploaded-by: {}", uploader_name);

        // Upsert the tag and get its ID
        let tags_url = client.build_url("/tags", &[]);
        let tag_id: Option<String> = {
            #[derive(Deserialize)]
            struct TagResponse {
                id: String,
            }

            let mut result = None;
            for attempt in 0..=2u32 {
                match client
                    .http_client
                    .put(&tags_url)
                    .header("x-api-key", upload_api_key)
                    .json(&serde_json::json!({ "tags": [tag_name] }))
                    .send()
                    .await
                {
                    Ok(r) if r.status().is_success() => {
                        if let Ok(tags) = r.json::<Vec<TagResponse>>().await {
                            result = tags.into_iter().next().map(|t| t.id);
                        }
                        break;
                    }
                    Ok(r) => {
                        let status = r.status();
                        let body = r.text().await.unwrap_or_default();
                        eprintln!(
                            "[{}] upsert-tag attempt {}/3: HTTP {} - {}",
                            asset_id,
                            attempt + 1,
                            status,
                            body
                        );
                    }
                    Err(e) => {
                        eprintln!("[{}] upsert-tag attempt {}/3: {}", asset_id, attempt + 1, e);
                    }
                }
                if attempt < 2 {
                    tokio::time::sleep(std::time::Duration::from_millis(500 * 2u64.pow(attempt)))
                        .await;
                }
            }
            result
        };

        // Apply the tag to the asset
        if let Some(tid) = tag_id {
            let tag_assets_url = client.build_url("/tags/assets", &[]);
            let ok = retry_request(&asset_id, "apply-tag", 2, false, || {
                client
                    .http_client
                    .put(&tag_assets_url)
                    .header("x-api-key", upload_api_key)
                    .json(&serde_json::json!({
                        "assetIds": [asset_id],
                        "tagIds": [tid]
                    }))
                    .send()
            })
            .await;
            if !ok {
                failures.push("apply-tag");
            }
        } else {
            eprintln!(
                "[{}] skipping tag application: failed to upsert tag '{}'",
                asset_id, tag_name
            );
            failures.push("upsert-tag");
        }
    }

    if failures.is_empty() {
        eprintln!("[{}] upload complete (all steps succeeded)", asset_id);
        StatusCode::OK.into_response()
    } else {
        eprintln!("[{}] upload partially failed: {:?}", asset_id, failures);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "Asset uploaded but post-processing failed: {}",
                failures.join(", ")
            ),
        )
            .into_response()
    }
}
