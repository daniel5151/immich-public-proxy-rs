use crate::immich_client::client::ImmichClient;
use crate::immich_client::client::METADATA_TIMEOUT;
use crate::immich_client::client::get_cookie_share_token;
use crate::security::{
    check_csrf, extract_client_ip, insert_private_no_store, is_https_request, is_safe_param,
    is_valid_share_token, random_hex_128, rate_limited_response, unlock_limiter,
    validate_uploader_name,
};
use axum::body::{Body, Bytes};
use axum::extract::ConnectInfo;
use axum::extract::DefaultBodyLimit;
use axum::extract::Form;
use axum::extract::Multipart;
use axum::extract::Path;
use axum::extract::Query;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::IntoResponse;
use axum::response::Redirect;
use axum::response::sse::Event;
use axum::response::sse::KeepAlive;
use axum::response::sse::Sse;
use serde::Deserialize;
use std::net::SocketAddr;

/// Maximum single-file upload payload size (default 256 MiB, configurable via `IPP_MAX_UPLOAD_BYTES`).
pub fn max_upload_bytes() -> usize {
    static MAX_BYTES: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *MAX_BYTES.get_or_init(|| {
        const DEFAULT_MAX: usize = 256 * 1024 * 1024; // 256 MiB
        const MIN_MAX: usize = 1024 * 1024; // 1 MiB floor
        std::env::var("IPP_MAX_UPLOAD_BYTES")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .map(|n| n.max(MIN_MAX))
            .unwrap_or(DEFAULT_MAX)
    })
}

pub trait ProxyRoutes {
    fn proxy_routes(self) -> Self;
}

impl<T: Clone + Send + Sync + 'static> ProxyRoutes for axum::Router<T> {
    fn proxy_routes(self) -> Self {
        // Allow 1 MiB headroom on Axum's route body limit for multipart framing overhead;
        // the streaming multipart filter enforces `max_upload_bytes()` strictly on payload bytes.
        let upload_route_limit = max_upload_bytes().saturating_add(1024 * 1024);

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
            axum::routing::post(upload_asset_handler)
                .layer(DefaultBodyLimit::max(upload_route_limit)),
        )
        // Finish beacon for the session-scoped SSE stream: the client hits this once
        // its upload loop ends so the stream can terminate promptly (see
        // `mark_upload_session_done_handler` / `mod upload_sessions`).
        .route(
            "/share/{key}/upload/finish",
            axum::routing::post(mark_upload_session_done_handler),
        )
        .route(
            "/share/{key}/status",
            axum::routing::get(upload_status_batch_handler),
        )
        // SSE variant of the batched status endpoint. Server-push alternative to the
        // poll loop above, kept side-by-side for comparison/testing (see
        // `upload_status_stream_handler`). NB: this route must be registered *before*
        // the `/status/{asset_id}` catch-all below, otherwise axum would match the
        // literal segment "stream" as an `asset_id`.
        .route(
            "/share/{key}/status/stream",
            axum::routing::get(upload_status_stream_handler),
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

fn shared_get_request(
    client: &ImmichClient,
    url: &str,
    share_token: Option<&str>,
    range: Option<&HeaderValue>,
) -> reqwest::RequestBuilder {
    let mut req = client.http_client.get(url);
    if let Some(token) = share_token.filter(|t| is_valid_share_token(t)) {
        req = req.header("cookie", format!("immich_shared_link_token={}", token));
    }
    if let Some(range) = range {
        req = req.header(header::RANGE, range.clone());
    }
    req
}

async fn fetch_shared_link_for_proxy(
    client: &ImmichClient,
    key: &str,
    share_token: Option<&str>,
    context: &str,
) -> Result<crate::immich_client::model::SharedLink, StatusCode> {
    match client.fetch_share_me(key, share_token).await {
        Ok((status, text)) if status.is_success() => {
            serde_json::from_str::<crate::immich_client::model::SharedLink>(&text).map_err(|e| {
                eprintln!(
                    "{}: failed to parse share link response for key {:?}: {}",
                    context, key, e
                );
                StatusCode::INTERNAL_SERVER_ERROR
            })
        }
        Ok((status, text)) => {
            eprintln!(
                "{}: share link request failed for key {:?}: {} — {}",
                context, key, status, text
            );
            Err(StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        }
        Err(e) => {
            eprintln!(
                "{}: upstream request failed for key {:?}: {}",
                context, key, e
            );
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

async fn resolve_canonical_share_key(
    client: &ImmichClient,
    key: &str,
    share_token: Option<&str>,
    context: &str,
) -> Option<String> {
    fetch_shared_link_for_proxy(client, key, share_token, context)
        .await
        .ok()
        .map(|link| link.key)
}

/// On a 401 from an asset endpoint accessed via slug, resolve the canonical share
/// key and retry the request with it.  Returns the retried response on success, or
/// the original response unchanged if no retry was applicable.
async fn retry_with_canonical_key(
    client: &ImmichClient,
    original_res: reqwest::Response,
    key: &str,
    share_token: Option<&str>,
    subpath: &str,
    extra_params: &[(&str, &str)],
    range: Option<&HeaderValue>,
    context: &str,
) -> Result<reqwest::Response, StatusCode> {
    if original_res.status() != StatusCode::UNAUTHORIZED {
        return Ok(original_res);
    }
    if let Some(canonical_key) =
        resolve_canonical_share_key(client, key, share_token, context).await
    {
        if canonical_key != key {
            let mut retry_params: Vec<(&str, &str)> = vec![("key", canonical_key.as_str())];
            retry_params.extend_from_slice(extra_params);
            let retry_url = client.build_url(subpath, &retry_params);
            match shared_get_request(client, &retry_url, share_token, range)
                .send()
                .await
            {
                Ok(res) => return Ok(res),
                Err(e) => {
                    eprintln!("{}: canonical-key retry failed: {}", context, e);
                    return Err(StatusCode::INTERNAL_SERVER_ERROR);
                }
            }
        }
    }
    Ok(original_res)
}

pub async fn unlock_share_handler(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(payload): Form<UnlockPayload>,
) -> impl IntoResponse {
    if !is_safe_param(&payload.key) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    if !check_csrf(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }

    let client_ip = extract_client_ip(&headers, peer.ip());
    if let Err(retry_after) = unlock_limiter().check(client_ip) {
        return rate_limited_response(retry_after, "Too many unlock attempts");
    }

    let client = ImmichClient::new();

    // Immich 3.0: authenticate via POST /shared-links/login, which returns the
    // SharedLinkResponseDto and sets an `immich_shared_link_token` cookie.
    let login_result = client
        .login_share_link(&payload.key, &payload.password)
        .await;

    match login_result {
        Ok(Some((token, text))) if is_valid_share_token(&token) => {
            use base64::Engine;
            let b64_key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&payload.key);
            let b64_token =
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token.as_bytes());

            let secure_flag = if is_https_request(&headers) {
                " Secure;"
            } else {
                ""
            };

            // Store the Immich shared-link token in the proxy's own cookie
            let cookie1 = format!(
                "immich_share_token_{}={};{} Path=/; HttpOnly; SameSite=Lax",
                b64_key, b64_token, secure_flag
            );
            let mut resp = Redirect::to(&format!("/share/{}", payload.key)).into_response();
            if let Ok(val) = HeaderValue::from_str(&cookie1) {
                resp.headers_mut().append(header::SET_COOKIE, val);
            }

            // If the real key differs from the slug/identifier used, also set a cookie
            // for the real key so both paths work
            if let Ok(link) = serde_json::from_str::<crate::immich_client::model::SharedLink>(&text)
            {
                if link.key != payload.key && is_safe_param(&link.key) {
                    let b64_real_key =
                        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&link.key);
                    let cookie2 = format!(
                        "immich_share_token_{}={};{} Path=/; HttpOnly; SameSite=Lax",
                        b64_real_key, b64_token, secure_flag
                    );
                    if let Ok(val) = HeaderValue::from_str(&cookie2) {
                        resp.headers_mut().append(header::SET_COOKIE, val);
                    }
                }
            }
            return resp;
        }
        Ok(_) => {
            // Auth failed (wrong password or invalid link)
        }
        Err(e) => {
            eprintln!(
                "unlock: login_share_link failed for key {:?}: {}",
                payload.key, e
            );
        }
    }

    Redirect::to(&format!("/share/{}", payload.key)).into_response()
}

/// Safe inert raster image MIME types allowed to display inline for non-original views.
/// Active document formats (e.g. `image/svg+xml`, `text/html`) are never in this allowlist
/// and are forced to `Content-Disposition: attachment` alongside a sandboxed CSP.
fn is_safe_inline_image_mime(content_type: Option<&str>) -> bool {
    let Some(ct) = content_type else {
        return false;
    };
    let mime = ct.split(';').next().unwrap_or("").trim();
    matches!(
        mime.to_ascii_lowercase().as_str(),
        "image/jpeg"
            | "image/jpg"
            | "image/png"
            | "image/webp"
            | "image/gif"
            | "image/avif"
            | "image/heic"
            | "image/heif"
    )
}

/// Rewrites a `Content-Disposition` value to force `attachment` while preserving any
/// safe filename parameters from upstream.
fn force_attachment_disposition(existing: Option<&HeaderValue>) -> HeaderValue {
    if let Some(val) = existing.and_then(|v| v.to_str().ok()) {
        let trimmed = val.trim();
        if trimmed.len() >= 10 && trimmed[..10].eq_ignore_ascii_case("attachment") {
            return HeaderValue::from_str(trimmed)
                .unwrap_or_else(|_| HeaderValue::from_static("attachment"));
        }
        if let Some(semi_idx) = trimmed.find(';') {
            let rewritten = format!("attachment{}", &trimmed[semi_idx..]);
            if let Ok(hv) = HeaderValue::from_str(&rewritten) {
                return hv;
            }
        }
    }
    HeaderValue::from_static("attachment")
}

/// Ensures `Cache-Control` for authenticated share responses cannot be stored in shared caches.
fn private_cache_control(existing: Option<&HeaderValue>) -> HeaderValue {
    if let Some(val) = existing.and_then(|v| v.to_str().ok()) {
        let directives: Vec<&str> = val
            .split(',')
            .map(str::trim)
            .filter(|d| !d.is_empty() && !d.eq_ignore_ascii_case("public"))
            .collect();
        let has_private = directives
            .iter()
            .any(|d| d.eq_ignore_ascii_case("private") || d.eq_ignore_ascii_case("no-store"));
        let mut out = String::new();
        if !has_private {
            out.push_str("private");
            if !directives.is_empty() {
                out.push_str(", ");
            }
        }
        out.push_str(&directives.join(", "));
        if let Ok(hv) = HeaderValue::from_str(&out) {
            return hv;
        }
    }
    HeaderValue::from_static("private, no-store")
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
    if !is_safe_param(&key)
        || !is_safe_param(&id)
        || !matches!(
            size_str.as_str(),
            "thumbnail" | "preview" | "fullsize" | "original"
        )
    {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let client = ImmichClient::new();
    let share_token = get_cookie_share_token(&headers, &key);

    let mut params: Vec<(&str, &str)> = vec![("key", key.as_str())];
    let mut extra_params: Vec<(&str, &str)> = Vec::new();

    let subpath = if size_str == "original" {
        format!("/assets/{}/original", id)
    } else {
        params.push(("size", size_str.as_str()));
        extra_params.push(("size", size_str.as_str()));
        format!("/assets/{}/thumbnail", id)
    };

    let url = client.build_url(&subpath, &params);
    let range = headers.get(header::RANGE);

    let mut res = match shared_get_request(&client, &url, share_token.as_deref(), range)
        .send()
        .await
    {
        Ok(res) => res,
        Err(e) => {
            eprintln!(
                "proxy_photo: upstream request failed for asset {:?}: {}",
                id, e
            );
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    res = match retry_with_canonical_key(
        &client,
        res,
        &key,
        share_token.as_deref(),
        &subpath,
        &extra_params,
        range,
        "proxy_photo",
    )
    .await
    {
        Ok(res) => res,
        Err(status) => return status.into_response(),
    };

    let upstream_ct = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    // Force `Content-Disposition: attachment` on all `/original` downloads, and on any
    // successful non-original response whose MIME type is not in the inert raster image
    // allowlist (e.g., `image/svg+xml`). Avoid forcing attachment on non-2xx upstream
    // error responses (e.g., 401/404 JSON) so opening a broken image URL directly in a
    // browser tab displays the error status rather than prompting a file download.
    let force_attachment = size_str == "original"
        || (res.status().is_success() && !is_safe_inline_image_mime(upstream_ct.as_deref()));

    let mut builder = axum::response::Response::builder().status(res.status());
    for (k, v) in res.headers() {
        if [
            "content-type",
            "content-length",
            "etag",
            "last-modified",
            "accept-ranges",
            "content-range",
        ]
        .contains(&k.as_str())
        {
            builder = builder.header(k.clone(), v.clone());
        }
    }

    if force_attachment {
        let disp = force_attachment_disposition(res.headers().get(header::CONTENT_DISPOSITION));
        builder = builder.header(header::CONTENT_DISPOSITION, disp);
    } else if let Some(disp) = res.headers().get(header::CONTENT_DISPOSITION) {
        builder = builder.header(header::CONTENT_DISPOSITION, disp.clone());
    }

    if share_token.is_some() {
        let cc = private_cache_control(res.headers().get(header::CACHE_CONTROL));
        builder = builder.header(header::CACHE_CONTROL, cc);
    } else if let Some(cc) = res.headers().get(header::CACHE_CONTROL) {
        builder = builder.header(header::CACHE_CONTROL, cc.clone());
    }

    // Sandbox proxied media so even active document types cannot execute scripts on our origin.
    builder = builder
        .header(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; sandbox"),
        )
        .header(header::VARY, HeaderValue::from_static("Cookie"));

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
    let share_token = get_cookie_share_token(&headers, &key);

    let params: Vec<(&str, &str)> = vec![("key", key.as_str())];

    let subpath = format!("/assets/{}/video/playback", id);
    let url = client.build_url(&subpath, &params);
    let range = headers.get(header::RANGE);

    let mut res = match shared_get_request(&client, &url, share_token.as_deref(), range)
        .send()
        .await
    {
        Ok(res) => res,
        Err(e) => {
            eprintln!(
                "proxy_video: upstream request failed for asset {:?}: {}",
                id, e
            );
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    res = match retry_with_canonical_key(
        &client,
        res,
        &key,
        share_token.as_deref(),
        &subpath,
        &[],
        range,
        "proxy_video",
    )
    .await
    {
        Ok(res) => res,
        Err(status) => return status.into_response(),
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
        ]
        .contains(&k.as_str())
        {
            builder = builder.header(k.clone(), v.clone());
        }
    }

    if share_token.is_some() {
        let cc = private_cache_control(res.headers().get(header::CACHE_CONTROL));
        builder = builder.header(header::CACHE_CONTROL, cc);
    } else if let Some(cc) = res.headers().get(header::CACHE_CONTROL) {
        builder = builder.header(header::CACHE_CONTROL, cc.clone());
    }

    builder = builder
        .header(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'; sandbox"),
        )
        .header(header::VARY, HeaderValue::from_static("Cookie"));

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
    let share_token = get_cookie_share_token(&headers, &key);

    let share =
        match fetch_shared_link_for_proxy(&client, &key, share_token.as_deref(), "download_all")
            .await
        {
            Ok(share) => share,
            Err(status) => return IntoResponse::into_response(status),
        };
    let canonical_key = share.key.clone();
    let params: Vec<(&str, &str)> = vec![("key", canonical_key.as_str())];

    if !share.allow_download.unwrap_or(false) {
        return IntoResponse::into_response(StatusCode::FORBIDDEN);
    }

    // Immich v3: SharedLinkResponseDto returns assets: [] for album shares.
    // Fetch album assets via timeline API when needed.
    let mut share_assets = share.assets;
    if share_assets.is_empty() {
        if let Some(ref album) = share.album {
            share_assets = match client
                .fetch_album_assets(&album.id, &canonical_key, share_token.as_deref())
                .await
            {
                Ok(assets) => assets,
                Err(e) => {
                    eprintln!(
                        "download_all: failed to fetch album assets for {:?}: {}",
                        key, e
                    );
                    return IntoResponse::into_response(StatusCode::INTERNAL_SERVER_ERROR);
                }
            };
        }
    }

    let title = share
        .description
        .clone()
        .or_else(|| share.album.as_ref().and_then(|a| a.album_name.clone()))
        .unwrap_or_else(|| {
            if share.r#type.as_deref() == Some("INDIVIDUAL") {
                share_assets
                    .first()
                    .and_then(|a| a.original_file_name.clone())
                    .unwrap_or_else(|| "shared_assets".to_string())
            } else {
                "shared_assets".to_string()
            }
        });
    let sanitized_title = title.replace(|c: char| !c.is_alphanumeric() && c != '-', "_");
    let filename = format!("{}.zip", sanitized_title);

    const MAX_DOWNLOAD_ASSETS: usize = 5000;
    let allowed_ids: std::collections::HashSet<&str> =
        share_assets.iter().map(|a| a.id.as_str()).collect();

    let asset_ids: Vec<String> = if let Some(ids_str) = query.asset_ids {
        let mut seen = std::collections::HashSet::new();
        ids_str
            .split(',')
            .map(str::trim)
            .filter(|s| is_safe_param(s) && allowed_ids.contains(*s) && seen.insert(*s))
            .take(MAX_DOWNLOAD_ASSETS)
            .map(|s| s.to_string())
            .collect()
    } else {
        share_assets
            .iter()
            .take(MAX_DOWNLOAD_ASSETS)
            .map(|a| a.id.clone())
            .collect()
    };

    if asset_ids.is_empty() {
        return IntoResponse::into_response(StatusCode::NOT_FOUND);
    }

    let payload = serde_json::json!({
        "assetIds": asset_ids
    });

    let download_url = client.build_url("/download/archive", &params);
    let mut dl_req = client.http_client.post(&download_url).json(&payload);
    if let Some(token) = share_token.as_deref().filter(|t| is_valid_share_token(t)) {
        dl_req = dl_req.header("cookie", format!("immich_shared_link_token={}", token));
    }
    let res = match dl_req.send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            eprintln!(
                "download_all: archive download failed for key {:?}: {}",
                key,
                r.status()
            );
            return IntoResponse::into_response(StatusCode::INTERNAL_SERVER_ERROR);
        }
        Err(e) => {
            eprintln!(
                "download_all: upstream archive request failed for key {:?}: {}",
                key, e
            );
            return IntoResponse::into_response(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let mut resp_headers = HeaderMap::new();
    resp_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zip"),
    );
    let ascii_filename: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded_filename = urlencoding::encode(&filename);
    resp_headers.insert(
        header::CONTENT_DISPOSITION,
        format!(
            "attachment; filename=\"{}\"; filename*=UTF-8''{}",
            ascii_filename, encoded_filename
        )
        .parse()
        .unwrap(),
    );
    insert_private_no_store(&mut resp_headers);

    (resp_headers, Body::from_stream(res.bytes_stream())).into_response()
}

const MAX_TAG_CACHE_ENTRIES: usize = 4096;
const MAX_PROCESSED_ASSETS: usize = 4096;

#[derive(Clone)]
struct ProcessedAsset {
    uploader_name: String,
    canonical_key: String,
    timestamp: std::time::Instant,
}

static ADDED_ALBUMS: std::sync::OnceLock<parking_lot::RwLock<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();
static TAG_CACHE: std::sync::OnceLock<
    parking_lot::RwLock<std::collections::HashMap<String, String>>,
> = std::sync::OnceLock::new();
// Per-cache-key single-flight locks for tag creation. Without this, a burst of
// concurrent uploads from a brand-new uploader all miss the cache and race to
// POST /tags for the SAME child tag, tripping Immich's `tag_userId_value_uq`
// unique constraint. The losing tasks then resolve the id inconsistently and
// some asset->tag links are silently lost. Serializing creation per (parent,name)
// means exactly one task creates the tag and the rest await, then read the cache.
static TAG_LOCKS: std::sync::OnceLock<
    parking_lot::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
> = std::sync::OnceLock::new();
static PROCESSED_ASSETS: std::sync::OnceLock<
    parking_lot::RwLock<std::collections::HashMap<String, ProcessedAsset>>,
> = std::sync::OnceLock::new();

/// Invalidates both the incoming `request_key` (which may be a slug) and the
/// resolved `canonical_key` in `share_cache`, ensuring in-flight rebuilds started
/// under a slug observe the generation bump even when `share_cache` is empty.
fn invalidate_share_keys(request_key: &str, canonical_key: &str) {
    crate::api::get_share_details::share_cache::invalidate(request_key);
    if canonical_key != request_key {
        crate::api::get_share_details::share_cache::invalidate(canonical_key);
    }
}

fn record_processed_asset(asset_id: String, uploader_name: String, canonical_key: String) {
    let cache =
        PROCESSED_ASSETS.get_or_init(|| parking_lot::RwLock::new(std::collections::HashMap::new()));
    let mut write_guard = cache.write();

    // Clean up stale entries to prevent memory leaks from abandoned status polls,
    // and enforce a hard capacity ceiling (MAX_PROCESSED_ASSETS) via oldest-first eviction.
    let now = std::time::Instant::now();
    let expiry = std::time::Duration::from_secs(600); // 10 minutes
    write_guard.retain(|_, entry| now.duration_since(entry.timestamp) < expiry);

    if !write_guard.contains_key(&asset_id) {
        while write_guard.len() >= MAX_PROCESSED_ASSETS {
            if let Some(oldest_id) = write_guard
                .iter()
                .min_by_key(|(_, e)| e.timestamp)
                .map(|(id, _)| id.clone())
            {
                write_guard.remove(&oldest_id);
            } else {
                break;
            }
        }
    }

    write_guard.insert(
        asset_id,
        ProcessedAsset {
            uploader_name,
            canonical_key,
            timestamp: now,
        },
    );
}

// ---------------------------------------------------------------------------
// Upload-session registry — backs the SESSION-SCOPED SSE status stream.
//
// History / rationale (keep this — it's the whole reason this module exists):
// The first SSE cut took a fixed `ids=` list captured when the EventSource was
// opened. EventSource is GET-only with an immutable URL, so the id set could not
// grow on an open stream; the client therefore had to wait until *every* upload
// had been dispatched before opening the stream with the final id list. That
// defeated the entire point of progressive appearance — time-to-first-photo on a
// big drop was gated on the slowest-to-dispatch upload.
//
// A WebSocket was considered for the dynamic client->server channel and rejected:
// a WS upgrade reintroduces the edge-layer fragility (1006 drops behind the
// cloudflared/CrowdSec proxy) that plain-HTTP SSE/polling survive cleanly, and
// EventSource gives auto-reconnect for free. So instead we keep SSE (HTTP-only,
// robust through the edge) and move pending-set OWNERSHIP server-side:
//
//   * Each upload is tagged with a client-generated session token.
//   * The stream is opened for that *session*, not a frozen id list.
//   * Every tick the stream re-reads the session's CURRENT pending set, so assets
//     uploaded *after* the stream opened are picked up automatically — no URL
//     churn, no second connection, no client->server push channel.
//
// The session token is minted by the client (crypto.randomUUID / getRandomValues)
// and passed as the `session` query param on each upload POST and on the stream
// open. It's validated with the same `is_safe_param` rule as every other path
// segment (a UUID is alphanumeric + '-', which that rule already permits).
//
// Security & memory invariants:
//   * Every session is bound to the resolved `canonical_key` of the share it was
//     created under; operations from a different share key are rejected so one
//     share cannot observe, hijack, or terminate another share's session.
//   * Bounded to `MAX_SESSIONS` active sessions and `MAX_PENDING_PER_SESSION`
//     pending assets per session, with lazy TTL sweeping + oldest eviction.
// ---------------------------------------------------------------------------
mod upload_sessions {
    use std::collections::{HashMap, HashSet};
    use std::time::{Duration, Instant};

    const MAX_SESSIONS: usize = 512;
    const MAX_PENDING_PER_SESSION: usize = 1024;

    struct Session {
        /// Canonical share key that owns this session (enforces cross-share isolation).
        canonical_key: String,
        /// Asset ids uploaded under this session not yet resolved by a stream.
        /// GROWS as uploads land mid-stream; shrinks as the stream resolves them.
        pending: HashSet<String>,
        /// Set once the client's finish beacon fires: no more uploads are coming.
        /// The stream ends when this is true AND `pending` has drained.
        uploads_finished: bool,
        /// Creation time, for TTL sweeping of abandoned sessions.
        created: Instant,
    }

    static SESSIONS: std::sync::OnceLock<parking_lot::RwLock<HashMap<String, Session>>> =
        std::sync::OnceLock::new();

    /// Abandoned-session TTL. Generous (matches PROCESSED_ASSETS' 10-min expiry) so a
    /// slow-processing batch or a brief reconnect gap never drops a live session; the
    /// stream's own ~120s wall-clock is the real per-connection bound.
    const SESSION_TTL: Duration = Duration::from_secs(600);

    fn map() -> &'static parking_lot::RwLock<HashMap<String, Session>> {
        SESSIONS.get_or_init(|| parking_lot::RwLock::new(HashMap::new()))
    }

    /// Register a freshly-uploaded asset id under `session` (scoped to `canonical_key`),
    /// creating the session on first use. Also lazily sweeps sessions older than the TTL
    /// and enforces `MAX_SESSIONS` / `MAX_PENDING_PER_SESSION` caps.
    pub(super) fn add_pending(canonical_key: &str, session: &str, asset_id: &str) {
        let mut guard = map().write();
        let now = Instant::now();
        guard.retain(|_, s| now.duration_since(s.created) < SESSION_TTL);

        if let Some(existing) = guard.get_mut(session) {
            if existing.canonical_key != canonical_key {
                // Reject cross-share session collision/hijack
                return;
            }
            if existing.pending.len() < MAX_PENDING_PER_SESSION {
                existing.pending.insert(asset_id.to_string());
            }
            return;
        }

        while guard.len() >= MAX_SESSIONS {
            if let Some(oldest_key) = guard
                .iter()
                .min_by_key(|(_, s)| s.created)
                .map(|(k, _)| k.clone())
            {
                guard.remove(&oldest_key);
            } else {
                break;
            }
        }

        let mut pending = HashSet::new();
        pending.insert(asset_id.to_string());
        guard.insert(
            session.to_string(),
            Session {
                canonical_key: canonical_key.to_string(),
                pending,
                uploads_finished: false,
                created: now,
            },
        );
    }

    /// Mark a session's uploads complete (if owned by `canonical_key`). No-op if the
    /// session is unknown or belongs to a different share.
    pub(super) fn mark_finished(canonical_key: &str, session: &str) {
        if let Some(s) = map().write().get_mut(session) {
            if s.canonical_key == canonical_key {
                s.uploads_finished = true;
            }
        }
    }

    /// Snapshot the session's current pending ids (if owned by `canonical_key`).
    /// `None` if the session is unknown, already cleaned up, or belongs to another share.
    pub(super) fn snapshot_pending(canonical_key: &str, session: &str) -> Option<Vec<String>> {
        map()
            .read()
            .get(session)
            .filter(|s| s.canonical_key == canonical_key)
            .map(|s| s.pending.iter().cloned().collect())
    }

    /// True once the client has signaled no more uploads are coming for this share's session.
    /// Unknown or cross-share sessions report false (keep waiting until wall-clock guard fires).
    pub(super) fn is_finished(canonical_key: &str, session: &str) -> bool {
        map()
            .read()
            .get(session)
            .filter(|s| s.canonical_key == canonical_key)
            .map(|s| s.uploads_finished)
            .unwrap_or(false)
    }

    /// Drop an id from the session's pending set once the stream has resolved it.
    pub(super) fn remove_pending(canonical_key: &str, session: &str, asset_id: &str) {
        if let Some(s) = map().write().get_mut(session) {
            if s.canonical_key == canonical_key {
                s.pending.remove(asset_id);
            }
        }
    }

    /// Remove the whole session (called when its owning stream terminates).
    pub(super) fn remove_session(canonical_key: &str, session: &str) {
        let mut guard = map().write();
        if guard
            .get(session)
            .is_some_and(|s| s.canonical_key == canonical_key)
        {
            guard.remove(session);
        }
    }
}

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

    // Fast path: already cached.
    {
        let read_guard = cache.read();
        if let Some(id) = read_guard.get(&cache_key) {
            return Some(id.clone());
        }
    }

    // Single-flight: serialize creation for this exact (parent,name) so a burst of
    // concurrent uploads from a new uploader doesn't stampede POST /tags and trip
    // Immich's tag_userId_value_uq unique constraint. We grab (or create) an async
    // mutex keyed by cache_key, then do the list/create under it. Other tasks for
    // the same key block here and, once we release, hit the cache fast-path above
    // via the post-lock re-check below.
    let locks = TAG_LOCKS.get_or_init(|| parking_lot::Mutex::new(std::collections::HashMap::new()));
    let key_lock = {
        let mut guard = locks.lock();
        if guard.len() >= MAX_TAG_CACHE_ENTRIES && !guard.contains_key(&cache_key) {
            // Reclaim uncontended single-flight lock entries (strong_count == 1) to stay bounded.
            guard.retain(|_, v| std::sync::Arc::strong_count(v) > 1);
        }
        guard
            .entry(cache_key.clone())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    let _create_guard = key_lock.lock().await;

    // Re-check the cache: another task holding the lock before us may have just
    // created and cached the tag.
    {
        let read_guard = cache.read();
        if let Some(id) = read_guard.get(&cache_key) {
            return Some(id.clone());
        }
    }

    let upload_key = client.upload_api_key.as_ref()?;

    // Step 1: List all tags and bulk-cache them (authoritative read).
    if let Some(id) = list_and_cache_tags(client, upload_key, cache, &cache_key).await {
        return Some(id);
    }

    // Step 2: Create the tag. Under the single-flight lock this should be the only
    // in-flight POST for this key from this process, but Immich may still 4xx if the
    // tag exists from a prior run or another client — which we treat as "go re-read".
    let post_url = client.build_url("/tags", &[]);
    let create_body = serde_json::json!({
        "name": name,
        "parentId": parent_id,
    });

    let create_res = client
        .http_client
        .post(&post_url)
        .timeout(METADATA_TIMEOUT)
        .header("x-api-key", upload_key)
        .json(&create_body)
        .send()
        .await;

    match create_res {
        Ok(res) if res.status().is_success() || res.status() == StatusCode::CREATED => {
            if let Ok(created_tag) = res.json::<crate::immich_client::model::Tag>().await {
                let mut write_guard = cache.write();
                if write_guard.len() >= MAX_TAG_CACHE_ENTRIES
                    && !write_guard.contains_key(&cache_key)
                {
                    if let Some(evict_key) = write_guard.keys().next().cloned() {
                        write_guard.remove(&evict_key);
                    }
                }
                write_guard.insert(cache_key.clone(), created_tag.id.clone());
                return Some(created_tag.id);
            }
            eprintln!(
                "upload: tag create for {:?} returned success but body failed to parse; re-reading /tags",
                cache_key
            );
        }
        Ok(res) => {
            // Most commonly a duplicate-key conflict: the tag already exists. This is
            // expected and recoverable — re-read /tags to resolve the authoritative id.
            // Do NOT swallow it silently; we want this visible if recovery then fails.
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            eprintln!(
                "upload: tag create for {:?} returned status {} (likely already exists) — recovering via /tags re-read; body: {}",
                cache_key,
                status,
                body.chars().take(200).collect::<String>()
            );
        }
        Err(e) => {
            eprintln!(
                "upload: tag create request for {:?} failed at transport level: {} — recovering via /tags re-read",
                cache_key, e
            );
        }
    }

    // Step 3: Recovery re-read. Retry a couple of times to ride out read-after-write
    // / brief propagation, since this is the path that previously lost the id under load.
    for attempt in 1..=3 {
        if let Some(id) = list_and_cache_tags(client, upload_key, cache, &cache_key).await {
            return Some(id);
        }
        if attempt < 3 {
            tokio::time::sleep(std::time::Duration::from_millis(150 * attempt)).await;
        }
    }

    eprintln!(
        "upload: FAILED to get-or-create tag {:?} after create + 3 recovery reads",
        cache_key
    );
    None
}

/// GET /tags, bulk-cache every tag by (parent,name) key (bounded to
/// `MAX_TAG_CACHE_ENTRIES`), and return the id for `cache_key` if present.
/// Returns None on transport/parse failure or cache miss.
async fn list_and_cache_tags(
    client: &ImmichClient,
    upload_key: &str,
    cache: &parking_lot::RwLock<std::collections::HashMap<String, String>>,
    cache_key: &str,
) -> Option<String> {
    let get_url = client.build_url("/tags", &[]);
    let res = client
        .http_client
        .get(&get_url)
        .timeout(METADATA_TIMEOUT)
        .header("x-api-key", upload_key)
        .send()
        .await
        .ok()?;

    if !res.status().is_success() {
        return None;
    }

    let tags = res
        .json::<Vec<crate::immich_client::model::Tag>>()
        .await
        .ok()?;

    let mut write_guard = cache.write();
    if tags.len() >= MAX_TAG_CACHE_ENTRIES || write_guard.len() + tags.len() > MAX_TAG_CACHE_ENTRIES
    {
        write_guard.clear();
    }
    let mut found = None;
    for tag in &tags {
        let key = match &tag.parent_id {
            Some(p_id) => format!("{}:{}", p_id, tag.name),
            None => format!("root:{}", tag.name),
        };
        if key == cache_key {
            found = Some(tag.id.clone());
        }
        // Overwrite: /tags is authoritative, so a fresh listing wins over stale entries.
        if write_guard.len() < MAX_TAG_CACHE_ENTRIES {
            write_guard.insert(key, tag.id.clone());
        }
    }
    found.or_else(|| write_guard.get(cache_key).cloned())
}

static IMMICH_API_SEMAPHORE: std::sync::OnceLock<tokio::sync::Semaphore> =
    std::sync::OnceLock::new();
static TAG_GUARD_SEMAPHORE: std::sync::OnceLock<tokio::sync::Semaphore> =
    std::sync::OnceLock::new();
static SSE_SEMAPHORE: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
    std::sync::OnceLock::new();

/// Tri-state result of reading an asset's tag list back from Immich.
///
/// The distinction matters for the deferred guard: it must only re-issue a tag
/// link when the tag is *confirmed absent*. A transport error, timeout, or parse
/// failure is `Unknown`, NOT absent — re-applying on an inconclusive read races
/// Immich into a `tag_asset_pkey` duplicate-key 500 (the link was actually there;
/// our read just failed). `Unknown` therefore means "don't act this tick".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagState {
    Present,
    Absent,
    Unknown,
}

/// Authoritative read of whether `asset_id` carries `tag_id`. Returns `Present`
/// or `Absent` only on a successful GET we could parse; any transport/parse error
/// or non-success status is `Unknown` (caller must not treat it as absent).
async fn asset_tag_state(client: &ImmichClient, asset_id: &str, tag_id: &str) -> TagState {
    let url = client.build_url(&format!("/assets/{}", asset_id), &[]);
    let res = client
        .http_client
        .get(&url)
        .timeout(METADATA_TIMEOUT)
        .header("x-api-key", client.upload_api_key.as_ref().unwrap())
        .send()
        .await;

    match res {
        Ok(r) if r.status().is_success() => {
            match r.json::<crate::immich_client::model::Asset>().await {
                Ok(asset_info) => {
                    let has = asset_info
                        .tags
                        .as_ref()
                        .map(|tags| tags.iter().any(|t| t.id == tag_id))
                        .unwrap_or(false);
                    if has {
                        TagState::Present
                    } else {
                        TagState::Absent
                    }
                }
                Err(_) => TagState::Unknown,
            }
        }
        _ => TagState::Unknown,
    }
}

/// Convenience wrapper for callers that only need "confirmed present". A `Present`
/// state is `true`; both `Absent` and `Unknown` are `false` ("not confirmed"), so
/// the synchronous PUT+verify path keeps re-linking within its bounded cycles.
async fn asset_has_tag(client: &ImmichClient, asset_id: &str, tag_id: &str) -> bool {
    asset_tag_state(client, asset_id, tag_id).await == TagState::Present
}

/// Outcome of issuing `PUT /tags/{id}/assets` for a single asset, decoded from the
/// per-asset response body rather than the HTTP status alone.
///
/// Immich returns `200 OK` with a body like `[{"id":..,"success":false,"error":
/// "duplicate"}]` when the link already exists — that is NOT a fresh application
/// and must not be reported as a restore. A `5xx`/transport failure or a genuine
/// per-asset error is `Failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelinkOutcome {
    /// The tag link was freshly created by this call.
    Applied,
    /// The link already existed (`"error":"duplicate"`); nothing changed.
    AlreadyPresent,
    /// The call failed (non-success status, transport error, or per-asset error).
    Failed,
}

/// Issue `PUT /tags/{id}/assets` for one asset and classify the result from the
/// per-asset body. Distinguishes a genuine fresh link (`Applied`) from a no-op
/// duplicate (`AlreadyPresent`) so callers don't mistake an already-present tag
/// for a restore (and don't fire spurious cache invalidations).
async fn relink_tag(client: &ImmichClient, asset_id: &str, tag_id: &str) -> RelinkOutcome {
    let tag_url = client.build_url(&format!("/tags/{}/assets", tag_id), &[]);
    let res = client
        .http_client
        .put(&tag_url)
        .timeout(METADATA_TIMEOUT)
        .header("x-api-key", client.upload_api_key.as_ref().unwrap())
        .json(&serde_json::json!({ "ids": [asset_id] }))
        .send()
        .await;

    match res {
        Ok(r) if r.status().is_success() => {
            // Body is an array of per-asset results: [{id, success, error?}].
            match r.json::<serde_json::Value>().await {
                Ok(body) => {
                    let entry = body.as_array().and_then(|a| {
                        a.iter()
                            .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(asset_id))
                            .or_else(|| a.first())
                    });
                    let success = entry
                        .and_then(|e| e.get("success").and_then(|v| v.as_bool()))
                        .unwrap_or(false);
                    let err = entry
                        .and_then(|e| e.get("error").and_then(|v| v.as_str()))
                        .unwrap_or("");
                    if success {
                        RelinkOutcome::Applied
                    } else if err.eq_ignore_ascii_case("duplicate") {
                        RelinkOutcome::AlreadyPresent
                    } else {
                        eprintln!(
                            "relink: asset {} tag {} PUT 200 but success=false error={:?}",
                            asset_id, tag_id, err
                        );
                        RelinkOutcome::Failed
                    }
                }
                // 200 with an unparseable body: the op was accepted, but don't claim
                // a fresh apply we can't prove.
                Err(_) => RelinkOutcome::AlreadyPresent,
            }
        }
        Ok(r) => {
            eprintln!(
                "relink: asset {} tag {} PUT returned status {}",
                asset_id,
                tag_id,
                r.status()
            );
            RelinkOutcome::Failed
        }
        Err(e) => {
            eprintln!(
                "relink: asset {} tag {} PUT request failed: {}",
                asset_id, tag_id, e
            );
            RelinkOutcome::Failed
        }
    }
}

async fn tag_and_associate_asset(
    client: &ImmichClient,
    asset_id: &str,
    album_id: &str,
    uploader_name: &str,
) -> bool {
    // Concurrency for the tag/associate background work. Defaults to 4 so multiple
    // simultaneous uploads can be processed in parallel; the only shared mutable
    // state (TAG_CACHE / ADDED_ALBUMS) is independently locked, and the tag-create
    // path already handles concurrent creation via single-flight + POST-conflict re-query.
    let sem = IMMICH_API_SEMAPHORE.get_or_init(|| {
        let permits = std::env::var("IPP_UPLOAD_CONCURRENCY")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n >= 1)
            .unwrap_or(4);
        tokio::sync::Semaphore::new(permits)
    });
    let _permit = sem.acquire().await.ok();

    let mut trash_checked = false;
    let mut tagged = false;
    let mut added_to_album = false;

    // Retry loop: a handful of attempts is plenty; Immich settles read-after-write
    // quickly, and the per-step backoff below covers the rare transient case.
    for attempt in 1..=4 {
        // Step 1: Check trash status and restore if needed
        if !trash_checked {
            let get_asset_url = client.build_url(&format!("/assets/{}", asset_id), &[]);
            let asset_res = client
                .http_client
                .get(&get_asset_url)
                .timeout(METADATA_TIMEOUT)
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
                                .timeout(METADATA_TIMEOUT)
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

        // Step 2: Tag the asset with uploader name.
        //
        // Robustness model (this is the path that produced production orphan tags):
        // a PUT /tags/{id}/assets can return success:true while the asset->tag link
        // is later not present (observed under concurrent load through the real edge,
        // alongside Immich tag_userId_value_uq duplicate-key errors). So we never
        // trust the PUT response alone: we PUT, then GET the asset and confirm the
        // tag is actually attached. If it isn't, we re-PUT and re-check. `tagged`
        // only flips true after a confirming read, so a vanished link self-heals
        // here instead of being reported as a false-positive success.
        if trash_checked && !tagged {
            let parent_tag_id = get_or_create_tag(client, "SharedBy", None).await;
            let child_tag_id = match &parent_tag_id {
                Some(p) => get_or_create_tag(client, uploader_name, Some(p)).await,
                None => None,
            };

            match (&parent_tag_id, &child_tag_id) {
                (Some(_), Some(child_tag_id)) => {
                    // Up to 3 PUT+verify cycles within this outer attempt. Each cycle
                    // re-issues the link if a fresh read shows it absent.
                    let mut confirmed = false;
                    for link_try in 1..=3 {
                        // Issue (or re-issue) the tag->asset link.
                        let tag_url =
                            client.build_url(&format!("/tags/{}/assets", child_tag_id), &[]);
                        let tag_res = client
                            .http_client
                            .put(&tag_url)
                            .timeout(METADATA_TIMEOUT)
                            .header("x-api-key", client.upload_api_key.as_ref().unwrap())
                            .json(&serde_json::json!({ "ids": [asset_id] }))
                            .send()
                            .await;

                        match tag_res {
                            Ok(res) if res.status().is_success() => { /* fall through to verify */ }
                            Ok(res) => {
                                eprintln!(
                                    "upload: tagging PUT failed for asset {} with status {} (outer {} link_try {})",
                                    asset_id,
                                    res.status(),
                                    attempt,
                                    link_try
                                );
                            }
                            Err(e) => {
                                eprintln!(
                                    "upload: tagging PUT request failed for asset {}: {} (outer {} link_try {})",
                                    asset_id, e, attempt, link_try
                                );
                            }
                        }

                        // Authoritative verify: read the asset and confirm the tag is
                        // attached. A short settle wait covers read-after-write.
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                        if asset_has_tag(client, asset_id, child_tag_id).await {
                            confirmed = true;
                            break;
                        }
                        eprintln!(
                            "upload: TAG-VERIFY-MISS asset {} tag {} (uploader {:?}) link absent after PUT (outer {} link_try {}) — retrying",
                            asset_id, child_tag_id, uploader_name, attempt, link_try
                        );
                        // brief backoff before the next PUT+verify cycle
                        tokio::time::sleep(std::time::Duration::from_millis(250 * link_try)).await;
                    }

                    if confirmed {
                        tagged = true;
                    }
                    // If not confirmed, leave `tagged=false`; the outer retry loop will
                    // re-enter this block, and the final INCOMPLETE log will fire so the
                    // miss is never silently reported as success.
                }
                (Some(_), None) => {
                    eprintln!(
                        "upload: could not resolve child tag {:?} for asset {} (outer attempt {})",
                        uploader_name, asset_id, attempt
                    );
                }
                (None, _) => {
                    eprintln!(
                        "upload: could not resolve parent tag 'SharedBy' for asset {} (outer attempt {})",
                        asset_id, attempt
                    );
                }
            }
        }

        // Step 3: Add to album
        if trash_checked && !added_to_album {
            let album_url = client.build_url(&format!("/albums/{}/assets", album_id), &[]);
            let album_res = client
                .http_client
                .put(&album_url)
                .timeout(METADATA_TIMEOUT)
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

    // [tag-debug] Always log the final outcome (previously only on failure) so every
    // asset's tag/album result can be correlated against the upload-accept line above.
    if !trash_checked || !tagged || !added_to_album {
        eprintln!(
            "upload: FINISHED asset {} INCOMPLETE: trash_checked={}, tagged={}, added_to_album={}",
            asset_id, trash_checked, tagged, added_to_album
        );
    } else {
        println!(
            "upload: FINISHED asset {} ok: tagged + added_to_album",
            asset_id
        );
    }

    trash_checked && tagged && added_to_album
}

/// Deferred tag guard: defends against Immich's async metadata-extraction job.
///
/// Immich (observed on 2.7.5) runs a metadata-extraction job ~1s after upload that
/// calls `replaceAssetTags(id, tags_parsed_from_file)`. For files with no embedded
/// keyword metadata (e.g. Pixel `PXL_*` JPEGs) the parsed set is empty, so the job
/// REPLACES the asset's whole tag set with `[]` — silently wiping the `SharedBy/...`
/// tag the proxy applied over the API. Because that wipe can land AFTER the
/// synchronous PUT+verify in `tag_and_associate_asset` has already confirmed the
/// tag, the upload is logged `FINISHED ok` yet ends up an orphan.
///
/// This guard runs detached, well past the synchronous path, and re-applies the tag
/// if it has vanished. It terminates only after seeing the tag present on two spaced
/// checks — i.e. the metadata job has already run and there is no pending wipe — so
/// it is robust even when extraction is delayed under load. It re-resolves the child
/// tag id itself (TAG_CACHE fast-path) to stay decoupled from the tagging path.
/// Upstream HTTP calls inside each tick are bounded by `TAG_GUARD_SEMAPHORE` (not held
/// across sleeps) so a large upload burst cannot storm Immich concurrently.
async fn deferred_tag_guard(
    client: &ImmichClient,
    asset_id: &str,
    uploader_name: &str,
    request_key: &str,
    canonical_key: &str,
) {
    // Guard is on by default; set IPP_TAG_GUARD=0 to disable.
    let enabled = std::env::var("IPP_TAG_GUARD")
        .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
        .unwrap_or(true);
    if !enabled {
        return;
    }

    // Inter-check delays in seconds. The first checks straddle the typical ~1s
    // extraction window; the later ones extend coverage when extraction is delayed
    // under load. Override with IPP_TAG_GUARD_SCHEDULE="2,4,8,16,30".
    let schedule: Vec<u64> = std::env::var("IPP_TAG_GUARD_SCHEDULE")
        .ok()
        .map(|s| {
            s.split(',')
                .filter_map(|p| p.trim().parse::<u64>().ok())
                .filter(|n| *n > 0)
                .collect::<Vec<_>>()
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec![2, 4, 8, 16, 30]);

    let guard_sem = TAG_GUARD_SEMAPHORE.get_or_init(|| tokio::sync::Semaphore::new(8));

    // Re-resolve the child tag id (cached after the synchronous tagging path ran).
    let child_tag_id = {
        let _permit = guard_sem.acquire().await.ok();
        let parent_tag_id = get_or_create_tag(client, "SharedBy", None).await;
        match &parent_tag_id {
            Some(p) => get_or_create_tag(client, uploader_name, Some(p)).await,
            None => None,
        }
    };
    let child_tag_id = match child_tag_id {
        Some(id) => id,
        None => {
            eprintln!(
                "guard: asset {} could not resolve child tag for uploader {:?}; skipping guard",
                asset_id, uploader_name
            );
            return;
        }
    };

    let mut consecutive_present = 0u8;
    // Tracks whether THIS guard actually re-created a link that Immich had wiped.
    // Only a real re-apply justifies invalidating the share cache. A no-op
    // duplicate (link was already there; our prior read was just inconclusive)
    // must NOT set this.
    let mut restored = false;
    for delay in &schedule {
        tokio::time::sleep(std::time::Duration::from_secs(*delay)).await;

        let _permit = guard_sem.acquire().await.ok();
        match asset_tag_state(client, asset_id, &child_tag_id).await {
            TagState::Present => {
                consecutive_present += 1;
                // Two spaced confirmations => extraction has run and link is stable.
                if consecutive_present >= 2 {
                    if restored {
                        // Attribution was genuinely restored after a wipe; refresh
                        // the share cache (both request slug and canonical key) so
                        // viewers see the corrected uploader.
                        invalidate_share_keys(request_key, canonical_key);
                        println!(
                            "guard: asset {} tag {} restored and now stable",
                            asset_id, child_tag_id
                        );
                    }
                    return;
                }
                continue;
            }
            TagState::Unknown => {
                // Inconclusive read (GET failed/timed out). Do NOT re-PUT: the link
                // may well be present, and a blind re-apply races Immich into a
                // tag_asset_pkey duplicate-key 500. Reset the streak and retry next
                // tick. Don't count this as a confirmed wipe either.
                consecutive_present = 0;
                eprintln!(
                    "guard: asset {} tag {} read inconclusive — deferring to next tick",
                    asset_id, child_tag_id
                );
                continue;
            }
            TagState::Absent => {
                // Confirmed absent — Immich wiped it (or it never settled). Re-apply
                // under the same tick permit.
                consecutive_present = 0;
                match relink_tag(client, asset_id, &child_tag_id).await {
                    RelinkOutcome::Applied => {
                        restored = true;
                        eprintln!(
                            "guard: asset {} tag {} (uploader {:?}) was MISSING — re-applied (metadata-extraction wipe)",
                            asset_id, child_tag_id, uploader_name
                        );
                    }
                    RelinkOutcome::AlreadyPresent => {
                        // Read said absent, write said duplicate: a benign race
                        // (the link reappeared between our GET and PUT). Treat as
                        // present; do not claim a restore or invalidate the cache.
                        consecutive_present = 1;
                        eprintln!(
                            "guard: asset {} tag {} re-link reported duplicate — link already present, no action",
                            asset_id, child_tag_id
                        );
                    }
                    RelinkOutcome::Failed => {
                        eprintln!(
                            "guard: asset {} tag {} re-apply failed — will retry next tick",
                            asset_id, child_tag_id
                        );
                    }
                }
            }
        }
    }

    // Schedule exhausted — report final state so a stubborn miss is never silent.
    let final_state = {
        let _permit = guard_sem.acquire().await.ok();
        asset_tag_state(client, asset_id, &child_tag_id).await
    };
    match final_state {
        TagState::Present => {
            if restored {
                invalidate_share_keys(request_key, canonical_key);
            }
            println!(
                "guard: asset {} tag {} present at end of guard schedule",
                asset_id, child_tag_id
            );
        }
        TagState::Unknown => {
            eprintln!(
                "guard: asset {} tag {} final read inconclusive — state unverified",
                asset_id, child_tag_id
            );
        }
        TagState::Absent => {
            eprintln!(
                "guard: asset {} tag {} STILL MISSING after guard schedule — orphan persists",
                asset_id, child_tag_id
            );
        }
    }
}

const ALLOWED_UPLOAD_FIELDS: &[&str] = &[
    "assetData",
    "deviceAssetId",
    "deviceId",
    "fileCreatedAt",
    "fileModifiedAt",
    "filename",
    "duration",
];
const MAX_UPLOAD_FIELDS: usize = 16;
const MAX_METADATA_FIELD_BYTES: usize = 4096;
/// Pre-buffer up to 64 KiB of validated multipart output before opening the upstream
/// HTTP POST to Immich. Because the frontend appends all small metadata fields
/// (`deviceAssetId`, `deviceId`, `fileCreatedAt`, `fileModifiedAt`) *before*
/// `assetData`, this guarantees that 100% of metadata fields and the opening
/// `assetData` header/chunk are validated before a single packet is sent to Immich.
/// Once `PREBUFFER_BYTES` is reached (for large photos/videos), we fire `ready_tx`
/// with the `mpsc::Receiver` and flush the remaining payload in O(1) RAM.
const PREBUFFER_BYTES: usize = 64 * 1024;

type MultipartStreamRx = tokio::sync::mpsc::Receiver<Result<Bytes, std::io::Error>>;
type MultipartReadyTx = tokio::sync::oneshot::Sender<Result<MultipartStreamRx, StatusCode>>;

struct MultipartSink {
    ready_tx: Option<MultipartReadyTx>,
    pre_buf: Vec<Bytes>,
    pre_bytes: usize,
    stream_tx: Option<tokio::sync::mpsc::Sender<Result<Bytes, std::io::Error>>>,
}

impl MultipartSink {
    fn new(ready_tx: MultipartReadyTx) -> Self {
        Self {
            ready_tx: Some(ready_tx),
            pre_buf: Vec::new(),
            pre_bytes: 0,
            stream_tx: None,
        }
    }

    async fn promote_to_stream(&mut self) -> bool {
        if self.stream_tx.is_some() {
            return true;
        }
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        if let Some(ready) = self.ready_tx.take() {
            if ready.send(Ok(rx)).is_err() {
                return false;
            }
        }
        for item in self.pre_buf.drain(..) {
            if tx.send(Ok(item)).await.is_err() {
                return false;
            }
        }
        self.stream_tx = Some(tx);
        true
    }

    async fn emit(&mut self, chunk: Bytes) -> bool {
        if let Some(ref tx) = self.stream_tx {
            return tx.send(Ok(chunk)).await.is_ok();
        }
        self.pre_bytes = self.pre_bytes.saturating_add(chunk.len());
        self.pre_buf.push(chunk);
        if self.pre_bytes >= PREBUFFER_BYTES {
            return self.promote_to_stream().await;
        }
        true
    }

    async fn fail(mut self, status: StatusCode, msg: &'static str) -> Result<(), StatusCode> {
        if let Some(ready) = self.ready_tx.take() {
            let _ = ready.send(Err(status));
        } else if let Some(tx) = self.stream_tx.take() {
            let _ = tx.send(Err(std::io::Error::other(msg))).await;
        }
        Err(status)
    }

    async fn finish(mut self) -> Result<(), StatusCode> {
        let _ = self.promote_to_stream().await;
        Ok(())
    }
}

fn sanitize_multipart_header_value(raw: &str) -> String {
    raw.chars()
        .filter(|&c| !c.is_control() && c != '"' && c != '\\')
        .take(255)
        .collect()
}

/// Streams an incoming `axum::extract::Multipart` into a bounded `mpsc` channel
/// re-encoded with `boundary`, enforcing a strict field-name allowlist, per-field
/// metadata size caps, and an overall `max_bytes` payload cap in O(1) memory.
///
/// Pre-buffers up to `PREBUFFER_BYTES` (64 KiB) before signaling `ready_tx` so
/// malformed requests or disallowed fields are rejected before opening an upstream
/// connection to Immich.
async fn pump_filtered_multipart(
    mut multipart: Multipart,
    boundary: &str,
    max_bytes: usize,
    ready_tx: MultipartReadyTx,
) -> Result<(), StatusCode> {
    let mut sink = MultipartSink::new(ready_tx);
    let mut field_count: usize = 0;
    let mut asset_data_count: usize = 0;
    let mut total_bytes: usize = 0;

    loop {
        let next = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(e) => {
                return sink.fail(e.status(), "malformed multipart body").await;
            }
        };

        field_count += 1;
        if field_count > MAX_UPLOAD_FIELDS {
            return sink
                .fail(StatusCode::BAD_REQUEST, "too many multipart fields")
                .await;
        }

        let Some(field_name) = next.name().map(str::to_string) else {
            return sink
                .fail(StatusCode::BAD_REQUEST, "missing multipart field name")
                .await;
        };

        if !ALLOWED_UPLOAD_FIELDS.contains(&field_name.as_str()) {
            eprintln!("upload: rejected disallowed multipart field {:?}", field_name);
            return sink
                .fail(StatusCode::BAD_REQUEST, "disallowed multipart field")
                .await;
        }

        let is_asset_data = field_name == "assetData";
        if is_asset_data {
            asset_data_count += 1;
            if asset_data_count > 1 {
                return sink
                    .fail(StatusCode::BAD_REQUEST, "duplicate assetData field")
                    .await;
            }
        }

        let mut header_buf = String::with_capacity(192);
        header_buf.push_str("--");
        header_buf.push_str(boundary);
        header_buf.push_str("\r\nContent-Disposition: form-data; name=\"");
        header_buf.push_str(&field_name);
        header_buf.push('"');
        if let Some(fname) = next.file_name() {
            let safe_fname = sanitize_multipart_header_value(fname);
            header_buf.push_str("; filename=\"");
            header_buf.push_str(&safe_fname);
            header_buf.push('"');
        }
        header_buf.push_str("\r\n");
        if let Some(ct) = next.content_type() {
            let safe_ct = sanitize_multipart_header_value(ct);
            if !safe_ct.is_empty() {
                header_buf.push_str("Content-Type: ");
                header_buf.push_str(&safe_ct);
                header_buf.push_str("\r\n");
            }
        }
        header_buf.push_str("\r\n");

        if !sink.emit(Bytes::from(header_buf)).await {
            // Upstream receiver dropped early; let caller inspect upstream response.
            return Ok(());
        }

        let mut field = next;
        let mut field_bytes: usize = 0;
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    field_bytes = field_bytes.saturating_add(chunk.len());
                    total_bytes = total_bytes.saturating_add(chunk.len());

                    if !is_asset_data && field_bytes > MAX_METADATA_FIELD_BYTES {
                        return sink
                            .fail(StatusCode::BAD_REQUEST, "metadata field too large")
                            .await;
                    }
                    if total_bytes > max_bytes {
                        return sink
                            .fail(StatusCode::PAYLOAD_TOO_LARGE, "upload exceeds max size")
                            .await;
                    }

                    if !sink.emit(chunk).await {
                        return Ok(());
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    return sink.fail(e.status(), "multipart chunk read error").await;
                }
            }
        }

        if !sink.emit(Bytes::from_static(b"\r\n")).await {
            return Ok(());
        }
    }

    if asset_data_count == 0 {
        return sink
            .fail(StatusCode::BAD_REQUEST, "missing assetData field")
            .await;
    }

    let closing = format!("--{}--\r\n", boundary);
    let _ = sink.emit(Bytes::from(closing)).await;
    sink.finish().await
}

pub async fn upload_asset_handler(
    headers: HeaderMap,
    Path(key): Path<String>,
    // Optional `?session=TOKEN` ties this upload to a session-scoped SSE stream (see
    // `mod upload_sessions`). `Query` reads only the URI, so it's safe to extract
    // before the body-consuming `Multipart`.
    Query(params): Query<std::collections::HashMap<String, String>>,
    multipart: Multipart,
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

    // Parse and strictly validate uploader name header
    let uploader_name = match headers.get("x-uploader-name").and_then(|h| h.to_str().ok()) {
        Some(val) if !val.is_empty() => match urlencoding::decode(val) {
            Ok(decoded) => match validate_uploader_name(&decoded) {
                Some(valid) => valid,
                None => return StatusCode::BAD_REQUEST.into_response(),
            },
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        },
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };

    let share_token = get_cookie_share_token(&headers, &key);

    // Validate share key first
    let share_link =
        match fetch_shared_link_for_proxy(&client, &key, share_token.as_deref(), "upload").await {
            Ok(link) => link,
            Err(status) => return status.into_response(),
        };
    let canonical_key = share_link.key.clone();

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

    // The upload (service-account) key may itself OWN the album, in which case it can
    // already add assets directly and does not need to be granted editor access. Only
    // attempt the admin editor-grant when the upload user is a *different* user from the
    // album owner. Note: after an API-key rotation the admin key can belong to a
    // different user than the album owner, so the grant 400s with "no album.share
    // access" — but that grant is unnecessary whenever the upload user is the owner
    // (the common case), so we skip it rather than retrying (and re-logging) per upload.
    let upload_user_is_owner = share_link
        .album
        .as_ref()
        .and_then(|a| a.get_owner())
        .map(|o| o.id == service_account_user_id)
        .unwrap_or(false);

    let is_added = {
        let cache =
            ADDED_ALBUMS.get_or_init(|| parking_lot::RwLock::new(std::collections::HashSet::new()));
        let read_guard = cache.read();
        read_guard.contains(album_id)
    };

    if !is_added && !upload_user_is_owner {
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
        if let Some(res) = add_res {
            let status = res.status();
            if status.is_success() || status == StatusCode::CONFLICT {
                // granted (or already a member)
            } else if status == StatusCode::BAD_REQUEST {
                let body = res.text().await.unwrap_or_default();
                if !body.contains("already") {
                    eprintln!(
                        "upload: could not grant service account editor on album {} (status {} — {}); \
                         proceeding (upload user may already have access via the share)",
                        album_id, status, body
                    );
                }
            } else {
                eprintln!(
                    "upload: could not grant service account editor on album {}: status {}",
                    album_id, status
                );
            }
        } else {
            eprintln!("upload: failed to send add user request (admin key missing)");
        }

        // Record the attempt regardless of outcome: the editor-grant is best-effort and
        // non-transient failures (e.g. admin key not the album owner) would just repeat
        // and spam the log on every subsequent upload. One attempt per album per process.
        let cache = ADDED_ALBUMS.get().unwrap();
        let mut write_guard = cache.write();
        write_guard.insert(album_id.clone());
    }

    // Stream-filter the multipart body with O(1) RAM overhead into Immich POST /assets.
    // Pre-validates leading metadata fields + first 64 KiB before opening the upstream
    // connection so invalid requests never touch Immich.
    let boundary = format!("ipp-boundary-{}", random_hex_128());
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let boundary_for_pump = boundary.clone();
    let max_bytes = max_upload_bytes();
    let pump_task = tokio::spawn(async move {
        pump_filtered_multipart(multipart, &boundary_for_pump, max_bytes, ready_tx).await
    });

    let rx = match ready_rx.await {
        Ok(Ok(rx)) => rx,
        Ok(Err(status)) => return status.into_response(),
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    let rx_stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });
    let reqwest_body = reqwest::Body::wrap_stream(rx_stream);

    // Forward the validated streamed request to Immich using the service account API key
    let url = client.build_url("/assets", &[]);
    let out_req = client
        .http_client
        .post(&url)
        .header("x-api-key", client.upload_api_key.as_ref().unwrap())
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={}", boundary),
        )
        .body(reqwest_body);

    let (pump_join, send_res) = tokio::join!(pump_task, out_req.send());

    if let Ok(Err(status)) = pump_join {
        return status.into_response();
    }

    let res = match send_res {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            let status = r.status();
            let body = r.text().await.unwrap_or_default();
            eprintln!(
                "upload: upstream rejected asset for key {:?}: {} — {}",
                key, status, body
            );
            return status.into_response();
        }
        Err(e) => {
            eprintln!("upload: upstream request failed for key {:?}: {}", key, e);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Parse the uploaded asset ID from the response
    #[derive(Deserialize)]
    struct AssetUploadResponse {
        id: String,
        // Immich returns "created" or "duplicate". Logged below to diagnose whether a
        // missing tag correlates with dedupe (a duplicate returns an existing asset id
        // possibly already mid-processing under a different uploader name).
        #[serde(default)]
        status: Option<String>,
    }

    let upload_resp: AssetUploadResponse = match res.json().await {
        Ok(v) => v,
        Err(e) => {
            eprintln!(
                "upload: failed to parse upload response for key {:?}: {}",
                key, e
            );
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let asset_id = upload_resp.id;

    // [tag-debug] Record how the upload resolved. A "duplicate" means Immich returned an
    // existing asset id; if that asset is concurrently processed under another uploader
    // name this is where divergence starts.
    println!(
        "upload: asset {} accepted (status={:?}, uploader={:?}, key={:?})",
        asset_id, upload_resp.status, uploader_name, key
    );

    // Session-scoped SSE support: if the client supplied an upload session token,
    // register this asset id under it (scoped to `canonical_key`) so an already-open
    // status stream picks it up on its next tick (see `mod upload_sessions`). This is
    // what lets photos stream back while later files in the same drop are still
    // uploading. The token is validated with the same is_safe_param rule as every path
    // segment; an absent/invalid token is a no-op, so the poll path and the legacy
    // ids= stream are unaffected.
    if let Some(session) = params.get("session") {
        if is_safe_param(session) {
            upload_sessions::add_pending(&canonical_key, session, &asset_id);
        }
    }

    // Album contents just changed; drop any cached share response (both request slug
    // and canonical key) so the next load rebuilds (IDEAS #6 invalidation).
    // Re-invalidated below once tagging/association completes, since that mutates the
    // asset's attribution too.
    invalidate_share_keys(&key, &canonical_key);

    // Spawn background task to tag and associate the asset, saving it to PROCESSED_ASSETS when done.
    let client_clone = client.clone();
    let asset_id_clone = asset_id.clone();
    let album_id_clone = album_id.clone();
    let uploader_name_clone = uploader_name.clone();
    let request_key_clone = key.clone();
    let canonical_key_clone = canonical_key.clone();

    tokio::spawn(async move {
        let success = tag_and_associate_asset(
            &client_clone,
            &asset_id_clone,
            &album_id_clone,
            &uploader_name_clone,
        )
        .await;

        // Attribution/album membership settled — invalidate both request key (slug) and
        // canonical key so any in-flight rebuild under either identifier is rejected and
        // the resolved uploader name is reflected on the next share load.
        invalidate_share_keys(&request_key_clone, &canonical_key_clone);

        if success {
            record_processed_asset(
                asset_id_clone.clone(),
                uploader_name_clone.clone(),
                canonical_key_clone.clone(),
            );

            // Deferred guard: Immich's async metadata-extraction job can REPLACE this
            // asset's tag set (with the empty set parsed from a keyword-less file) AFTER
            // the synchronous PUT+verify above already confirmed the tag, silently
            // orphaning it. Run a detached guard that re-checks across the extraction
            // window and re-applies the tag if it vanishes. Only runs once the initial
            // tag/associate succeeded, so it defends a known-good attribution.
            let guard_client = client_clone.clone();
            let guard_asset = asset_id_clone.clone();
            let guard_uploader = uploader_name_clone.clone();
            let guard_req_key = request_key_clone.clone();
            let guard_can_key = canonical_key_clone.clone();
            tokio::spawn(async move {
                deferred_tag_guard(
                    &guard_client,
                    &guard_asset,
                    &guard_uploader,
                    &guard_req_key,
                    &guard_can_key,
                )
                .await;
            });
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

// ---------------------------------------------------------------------------
// Upload-status validation + resolution helpers (shared by the single-asset,
// batched, and SSE status endpoints). See IDEAS.local.md "avoid slamming the
// status endpoint": a big album drop used to spawn one 500ms poll loop *per
// asset*, and every poll re-validated the share key via `fetch_share_me` (an
// upstream hit) on top of the thumbnail/asset reads. That amplified N uploads
// into ~3N upstream calls/sec. These helpers let the status endpoints validate
// once per request/stream and reuse a short-TTL permission cache across polls,
// collapsing the storm.
// ---------------------------------------------------------------------------

/// Permission and identity facts about a share link for the upload-status path,
/// including `canonical_key` for cross-share isolation.
#[derive(Clone)]
struct UploadLinkMeta {
    canonical_key: String,
    is_album: bool,
    allow_upload: bool,
    allow_download: bool,
}

/// Bounded short-TTL cache of share-link *permission* metadata, keyed by
/// `(share_key, Option<keyed_sha256(share_token)>)` for collision resistance.
///
/// Deliberately separate from `share_cache`: that one caches album *contents* and
/// is invalidated on every upload, so it would miss constantly during an active
/// drop. A link's *permissions* (type / allow_upload / allow_download) don't change
/// when its album contents do, so a stale-but-valid read within the TTL is safe and
/// lets a burst of status polls skip re-hitting Immich's `/shared-links/me`.
/// Only caches valid upload-enabled album links so probing random cookies on
/// read-only shares cannot bloat memory.
mod status_link_cache {
    use super::UploadLinkMeta;
    use crate::security::hash_token_keyed;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    const MAX_ENTRIES: usize = 512;

    type CacheKey = (String, Option<[u8; 32]>);

    static CACHE: std::sync::OnceLock<
        parking_lot::RwLock<HashMap<CacheKey, (UploadLinkMeta, Instant)>>,
    > = std::sync::OnceLock::new();

    fn ttl() -> Duration {
        static TTL: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
        *TTL.get_or_init(|| {
            let secs = std::env::var("IPP_TTL_STATUS_LINK_CACHE_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(60);
            Duration::from_secs(secs)
        })
    }

    fn cache_key(key: &str, share_token: Option<&str>) -> CacheKey {
        (key.to_string(), share_token.map(hash_token_keyed))
    }

    pub(super) fn get(key: &str, share_token: Option<&str>) -> Option<UploadLinkMeta> {
        let t = ttl();
        if t.is_zero() {
            return None;
        }
        let cache = CACHE.get()?;
        let guard = cache.read();
        // Check public (token-less) entry first, then token-scoped entry
        if let Some((meta, ts)) = guard.get(&(key.to_string(), None)) {
            if ts.elapsed() < t {
                return Some(meta.clone());
            }
        }
        if share_token.is_some() {
            if let Some((meta, ts)) = guard.get(&cache_key(key, share_token)) {
                if ts.elapsed() < t {
                    return Some(meta.clone());
                }
            }
        }
        None
    }

    pub(super) fn put(key: &str, share_token: Option<&str>, meta: UploadLinkMeta) {
        let t = ttl();
        if t.is_zero() {
            return;
        }
        let cache = CACHE.get_or_init(|| parking_lot::RwLock::new(HashMap::new()));
        let k = cache_key(key, share_token);
        let mut guard = cache.write();
        guard.retain(|_, (_, ts)| ts.elapsed() < t);
        if !guard.contains_key(&k) {
            while guard.len() >= MAX_ENTRIES {
                if let Some(oldest_k) = guard
                    .iter()
                    .min_by_key(|(_, (_, ts))| *ts)
                    .map(|(k, _)| k.clone())
                {
                    guard.remove(&oldest_k);
                } else {
                    break;
                }
            }
        }
        guard.insert(k, (meta, Instant::now()));
    }
}

/// Validate that `key` is an upload-enabled album share, reusing a short-TTL
/// permission cache so repeated status polls don't each hit Immich. On a cache
/// miss this calls `fetch_share_me` once and memoizes the result (when it is a
/// valid upload-enabled album link).
///
/// `Ok(meta)` means the key resolved; the caller still checks `is_album` /
/// `allow_upload`. `Err(status)` is an upstream/parse failure to surface verbatim.
async fn validate_upload_link(
    client: &ImmichClient,
    key: &str,
    share_token: Option<&str>,
) -> Result<UploadLinkMeta, StatusCode> {
    if let Some(meta) = status_link_cache::get(key, share_token) {
        return Ok(meta);
    }

    let share_link = fetch_shared_link_for_proxy(client, key, share_token, "status").await?;
    let is_password_protected = share_link.password.is_some();

    let meta = UploadLinkMeta {
        canonical_key: share_link.key,
        is_album: share_link.r#type.as_deref() == Some("ALBUM"),
        allow_upload: share_link.allow_upload.unwrap_or(false),
        allow_download: share_link.allow_download.unwrap_or(false),
    };

    // Only cache valid upload-enabled album shares; collapse public links under None token.
    if meta.is_album && meta.allow_upload {
        let cache_token = if is_password_protected {
            share_token
        } else {
            None
        };
        status_link_cache::put(key, cache_token, meta.clone());
    }
    Ok(meta)
}

/// Outcome of resolving a single just-uploaded asset.
enum UploadedAssetStatus {
    /// Still tagging/associating, or thumbnail not yet generated. Keep polling.
    Pending,
    /// Fully processed; carries the finished `SafeAsset`.
    Ready(Box<crate::dto::SafeAsset>),
    /// A non-recoverable upstream/parse error while resolving this asset.
    Error,
}

/// Checks if `asset_id` is in `PROCESSED_ASSETS` and belongs to `canonical_key`.
fn lookup_processed_uploader(asset_id: &str, canonical_key: &str) -> Option<String> {
    let cache = PROCESSED_ASSETS.get()?;
    let read_guard = cache.read();
    read_guard
        .get(asset_id)
        .filter(|entry| entry.canonical_key == canonical_key)
        .map(|entry| entry.uploader_name.clone())
}

fn remove_processed_asset(asset_id: &str, canonical_key: &str) {
    if let Some(cache) = PROCESSED_ASSETS.get() {
        let mut guard = cache.write();
        if guard
            .get(asset_id)
            .is_some_and(|e| e.canonical_key == canonical_key)
        {
            guard.remove(asset_id);
        }
    }
}

/// Resolve one asset by id: pending until it's in `PROCESSED_ASSETS` (under
/// `canonical_key`) *and* Immich has generated its thumbnail, then fetch the full
/// asset and build a `SafeAsset`. On `Ready` the asset is removed from
/// `PROCESSED_ASSETS`. No upstream calls are made for a still-pending or cross-share
/// asset (the in-memory map check short-circuits first), so polling a not-yet-ready
/// batch costs nothing upstream and cannot leak cross-share metadata.
async fn resolve_uploaded_asset(
    client: &ImmichClient,
    canonical_key: &str,
    asset_id: &str,
    allow_download: bool,
) -> UploadedAssetStatus {
    let Some(uploader_name) = lookup_processed_uploader(asset_id, canonical_key) else {
        return UploadedAssetStatus::Pending;
    };

    // Thumbnail generated yet?
    let get_thumb_url = client.build_url(
        &format!("/assets/{}/thumbnail", asset_id),
        &[("size", "thumbnail")],
    );
    let thumb_res = client
        .http_client
        .head(&get_thumb_url)
        .timeout(METADATA_TIMEOUT)
        .header("x-api-key", client.upload_api_key.as_ref().unwrap())
        .send()
        .await;
    match thumb_res {
        Ok(r) if r.status().is_success() => {}
        _ => return UploadedAssetStatus::Pending,
    }

    // Fetch final asset info to build a complete SafeAsset.
    let get_asset_url = client.build_url(&format!("/assets/{}", asset_id), &[]);
    let asset_res = client
        .http_client
        .get(&get_asset_url)
        .timeout(METADATA_TIMEOUT)
        .header("x-api-key", client.upload_api_key.as_ref().unwrap())
        .send()
        .await;
    let asset = match asset_res {
        Ok(r) if r.status().is_success() => {
            match r.json::<crate::immich_client::model::Asset>().await {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("status: failed to parse asset response: {}", e);
                    return UploadedAssetStatus::Error;
                }
            }
        }
        Ok(r) => {
            eprintln!("status: fetch asset returned status {}", r.status());
            return UploadedAssetStatus::Error;
        }
        Err(e) => {
            eprintln!("status: fetch asset request failed: {}", e);
            return UploadedAssetStatus::Error;
        }
    };

    let mut safe_asset = crate::dto::SafeAsset::from_base(asset);
    safe_asset.uploader_name = Some(uploader_name);
    safe_asset.uploader_is_fallback = false;
    if allow_download {
        safe_asset.download_url = Some(format!(
            "/share/photo/{}/{}/original",
            canonical_key, safe_asset.id
        ));
    }

    // Resolved — drop it from the pending map.
    remove_processed_asset(asset_id, canonical_key);
    UploadedAssetStatus::Ready(Box::new(safe_asset))
}

/// Batched upload-status endpoint: `GET /share/{key}/status?ids=a,b,c`.
///
/// Replaces the per-asset polling storm. The frontend keeps a single poll loop for
/// the whole in-flight batch and sends the still-pending ids; the share key is
/// validated once per request (cached across polls). Response:
/// `{ "ready": [SafeAsset...], "pending": ["id"...], "errored": ["id"...] }`.
pub async fn upload_status_batch_handler(
    headers: HeaderMap,
    Path(key): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    if !is_safe_param(&key) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    // Parse + validate the id list (comma-separated). Cap the batch to bound work.
    const MAX_BATCH: usize = 256;
    let ids: Vec<String> = params
        .get("ids")
        .map(|raw| {
            raw.split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if ids.len() > MAX_BATCH || ids.iter().any(|id| !is_safe_param(id)) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let client = ImmichClient::new();
    if client.upload_api_key.is_none() {
        return StatusCode::FORBIDDEN.into_response();
    }

    let share_token = get_cookie_share_token(&headers, &key);
    let meta = match validate_upload_link(&client, &key, share_token.as_deref()).await {
        Ok(m) => m,
        Err(status) => return status.into_response(),
    };
    if !meta.is_album || !meta.allow_upload {
        return StatusCode::FORBIDDEN.into_response();
    }

    let mut ready: Vec<crate::dto::SafeAsset> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    let mut errored: Vec<String> = Vec::new();
    for id in &ids {
        match resolve_uploaded_asset(&client, &meta.canonical_key, id, meta.allow_download).await {
            UploadedAssetStatus::Ready(a) => ready.push(*a),
            UploadedAssetStatus::Pending => pending.push(id.clone()),
            UploadedAssetStatus::Error => errored.push(id.clone()),
        }
    }

    #[derive(serde::Serialize)]
    struct BatchStatusResponse {
        ready: Vec<crate::dto::SafeAsset>,
        pending: Vec<String>,
        errored: Vec<String>,
    }
    let mut resp = (
        StatusCode::OK,
        axum::Json(BatchStatusResponse {
            ready,
            pending,
            errored,
        }),
    )
        .into_response();
    insert_private_no_store(resp.headers_mut());
    resp
}

/// Session-scoped SSE upload-status stream:
/// `GET /share/{key}/status/stream?session=TOKEN`
/// (legacy `?ids=a,b,c` is still accepted for back-compat — see below).
///
/// This is a *server-push* alternative to `upload_status_batch_handler`, kept
/// deliberately side-by-side with the poll endpoints (not a replacement) so the
/// two strategies can be A/B'd against the same backend. The motivation is the
/// same polling-storm / CrowdSec rationale documented on the batch handler: a big
/// album drop used to spray the edge with request bursts. The batch endpoint
/// already collapses that to one request per tick; SSE goes one step further and
/// collapses it to a *single* long-lived connection for the whole batch — the
/// client opens one stream and the server pushes each asset as it finishes, so
/// there are zero per-tick HTTP requests hitting the rate-limiter.
///
/// ## Why session-scoped (the important evolution over the first SSE cut)
/// The first version froze a `?ids=` list at stream-open time. Because EventSource
/// is GET-only with an immutable URL, the id set couldn't grow once the stream was
/// open, so the client had to defer opening the stream until *every* upload was
/// dispatched — killing time-to-first-photo, which is the entire point of
/// progressive appearance. A WebSocket (the obvious dynamic client->server channel)
/// was considered and rejected: a WS upgrade reintroduces the edge-layer fragility
/// (1006 drops behind cloudflared/CrowdSec) that plain-HTTP SSE survives, and loses
/// EventSource's free auto-reconnect. The fix keeps SSE but moves pending-set
/// ownership server-side: the upload handler tags each asset with a client-minted
/// `session` token, and this stream watches the session's CURRENT pending set every
/// tick. Assets uploaded *after* the stream opened are therefore picked up
/// automatically — no URL churn, no second connection, no client push channel. The
/// client can (and does) open the stream as soon as the first upload is dispatched.
///
/// Protocol (named SSE events; the client uses `addEventListener` per name):
///   - `ready`   — data is the finished `SafeAsset` as JSON (one event per asset).
///   - `errored` — data is the bare asset id string that failed to resolve.
///   - `done`    — terminal event; data is `{ "resolved": N, "pending": [id...] }`.
///     Sent exactly once, after which the server closes the stream.
///
/// ## Termination
/// A session stream can't end just because its pending set is momentarily empty —
/// more uploads in the same drop may still be in flight. It ends when EITHER:
///   * the client's finish beacon (`POST /share/{key}/upload/finish?session=...`)
///     has fired AND the pending set has drained, OR
///   * the ~120s wall-clock ceiling is hit (guards a stuck asset / lost beacon).
///
/// On termination we emit `done` and remove the session from the registry.
///
/// Validation mirrors the batch endpoint: `is_safe_param` on the key and token,
/// the 256-id `MAX_BATCH` cap on the legacy ids path, a global `SSE_SEMAPHORE`
/// concurrency cap (64 streams), and a single `validate_upload_link` call (which
/// reuses the short-TTL permission cache), so the share key is validated ONCE for
/// the life of the stream rather than once per poll. Inside the stream we reuse
/// `resolve_uploaded_asset` (scoped to `meta.canonical_key`) for every id — no
/// upstream-call logic is duplicated.
pub async fn upload_status_stream_handler(
    headers: HeaderMap,
    Path(key): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    if !is_safe_param(&key) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    const MAX_BATCH: usize = 256;

    // Two modes, primary first:
    //   * session mode (`?session=TOKEN`): server-tracked, dynamically-growing set.
    //   * legacy ids mode (`?ids=a,b,c`): a fixed list, kept for back-compat with the
    //     first SSE cut. Mutually exclusive; `session` wins if both are present.
    let session = params
        .get("session")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let legacy_ids: Vec<String> = params
        .get("ids")
        .map(|raw| {
            raw.split(',')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();

    // Validate whichever selector we got.
    match &session {
        Some(tok) => {
            if !is_safe_param(tok) {
                return StatusCode::BAD_REQUEST.into_response();
            }
        }
        None => {
            if legacy_ids.is_empty() {
                return StatusCode::BAD_REQUEST.into_response();
            }
            if legacy_ids.len() > MAX_BATCH || legacy_ids.iter().any(|id| !is_safe_param(id)) {
                return StatusCode::BAD_REQUEST.into_response();
            }
        }
    }

    let client = ImmichClient::new();
    if client.upload_api_key.is_none() {
        return StatusCode::FORBIDDEN.into_response();
    }

    // Validate the share key ONCE for the whole stream (reuses the permission cache).
    let share_token = get_cookie_share_token(&headers, &key);
    let meta = match validate_upload_link(&client, &key, share_token.as_deref()).await {
        Ok(m) => m,
        Err(status) => return status.into_response(),
    };
    if !meta.is_album || !meta.allow_upload {
        return StatusCode::FORBIDDEN.into_response();
    }

    // Bound global concurrent SSE streams to prevent connection/task exhaustion.
    const MAX_CONCURRENT_SSE: usize = 64;
    let sse_sem = SSE_SEMAPHORE
        .get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_SSE)));
    let Ok(sse_permit) = sse_sem.clone().try_acquire_owned() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };

    // Per-connection driver state. `stream::unfold` calls our async closure once per
    // yielded item; we keep a small output buffer because a single poll tick can
    // resolve several assets at once (each becomes its own SSE event) while unfold
    // yields one item per call.
    //
    // `session` distinguishes the two modes: `Some` => read the live pending set from
    // the registry each tick (it can grow); `None` => babysit the frozen `pending`
    // list captured from the legacy `ids=` param.
    struct StreamState {
        client: ImmichClient,
        canonical_key: String,
        allow_download: bool,
        session: Option<String>,
        pending: Vec<String>,
        resolved: usize,
        out: std::collections::VecDeque<Event>,
        start: std::time::Instant,
        delay_ms: u64,
        finished: bool,
        _permit: tokio::sync::OwnedSemaphorePermit,
    }

    // Same cadence spirit as the poll loop: 500ms -> 1s -> 2s (capped). A freshly
    // resolved asset resets the delay so its siblings stream in quickly.
    const MIN_DELAY_MS: u64 = 500;
    const MAX_DELAY_MS: u64 = 2000;
    const MAX_WALL_MS: u128 = 120_000;

    let init = StreamState {
        client,
        canonical_key: meta.canonical_key,
        allow_download: meta.allow_download,
        session,
        pending: legacy_ids, // empty in session mode; the registry is the source of truth there
        resolved: 0,
        out: std::collections::VecDeque::new(),
        start: std::time::Instant::now(),
        delay_ms: MIN_DELAY_MS,
        finished: false,
        _permit: sse_permit,
    };

    // The done payload reports how many resolved and which ids (if any) were still
    // pending at termination, so the client can stop waiting on them.
    #[derive(serde::Serialize)]
    struct DonePayload {
        resolved: usize,
        pending: Vec<String>,
    }

    let stream = futures_util::stream::unfold(init, move |mut st| async move {
        loop {
            // 1) Drain any already-buffered events first (one per unfold call).
            if let Some(ev) = st.out.pop_front() {
                return Some((Ok::<Event, std::convert::Infallible>(ev), st));
            }

            // 2) Nothing buffered and we've already sent `done` — end the stream.
            if st.finished {
                // Session mode: tear down the registry entry now that we're closing.
                if let Some(ref sess) = st.session {
                    upload_sessions::remove_session(&st.canonical_key, sess);
                }
                return None;
            }

            // 3) Refresh the pending set.
            //    * session mode: re-read the registry — this is where newly-uploaded
            //      ids (added after the stream opened) enter the working set.
            //    * legacy mode: `st.pending` is already the working set.
            let timed_out = st.start.elapsed().as_millis() >= MAX_WALL_MS;
            let uploads_finished = match st.session {
                Some(ref sess) => {
                    if let Some(live) = upload_sessions::snapshot_pending(&st.canonical_key, sess) {
                        st.pending = live;
                    }
                    upload_sessions::is_finished(&st.canonical_key, sess)
                }
                // Legacy ids mode has no finish beacon; it's "finished" by definition
                // (the id list never grows), so draining the list ends the stream.
                None => true,
            };

            // 4) Terminal conditions. In session mode we must NOT end on an empty
            //    pending set alone — more uploads may still be coming; we wait until
            //    the client's finish beacon has fired (uploads_finished) AND the set
            //    has drained. The wall clock is the backstop for a lost beacon or a
            //    permanently stuck asset.
            let drained_and_done = st.pending.is_empty() && uploads_finished;
            if drained_and_done || timed_out {
                if timed_out && !st.pending.is_empty() {
                    eprintln!("status stream: timed out for {} asset(s)", st.pending.len());
                }
                let payload = DonePayload {
                    resolved: st.resolved,
                    pending: std::mem::take(&mut st.pending),
                };
                let done = match Event::default().event("done").json_data(&payload) {
                    Ok(ev) => ev,
                    // Serializing a Vec<String>/usize cannot realistically fail; if it
                    // somehow does, fall back to a bare `done` so the client still closes.
                    Err(_) => Event::default().event("done").data(""),
                };
                st.finished = true;
                return Some((Ok(done), st));
            }

            // 5) If the set is momentarily empty but uploads aren't finished (session
            //    mode, waiting for the next file to land), idle one short tick rather
            //    than busy-looping, then re-check.
            if st.pending.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(MIN_DELAY_MS)).await;
                continue;
            }

            // 6) Otherwise wait one tick, then resolve the still-pending ids. Same
            //    backoff as the poll loop — but here it just paces a single in-process
            //    loop, it does NOT translate into client requests.
            tokio::time::sleep(std::time::Duration::from_millis(st.delay_ms)).await;
            st.delay_ms = (st.delay_ms * 2).min(MAX_DELAY_MS);

            let mut still_pending: Vec<String> = Vec::with_capacity(st.pending.len());
            let mut made_progress = false;
            // Iterate over a snapshot so we can rebuild `pending` cleanly.
            let snapshot = std::mem::take(&mut st.pending);
            for id in snapshot {
                match resolve_uploaded_asset(
                    &st.client,
                    &st.canonical_key,
                    &id,
                    st.allow_download,
                )
                .await
                {
                    UploadedAssetStatus::Ready(asset) => {
                        match Event::default().event("ready").json_data(&*asset) {
                            Ok(ev) => st.out.push_back(ev),
                            Err(e) => {
                                eprintln!("status stream: failed to serialize asset: {}", e);
                            }
                        }
                        // Resolved — drop it from the session registry too so a
                        // reconnect (which re-reads the registry) won't re-emit it.
                        if let Some(ref sess) = st.session {
                            upload_sessions::remove_pending(&st.canonical_key, sess, &id);
                        }
                        st.resolved += 1;
                        made_progress = true;
                    }
                    UploadedAssetStatus::Error => {
                        st.out
                            .push_back(Event::default().event("errored").data(id.clone()));
                        if let Some(ref sess) = st.session {
                            upload_sessions::remove_pending(&st.canonical_key, sess, &id);
                        }
                        made_progress = true;
                    }
                    UploadedAssetStatus::Pending => still_pending.push(id),
                }
            }
            // In session mode the registry is authoritative and may have grown while we
            // were resolving; don't clobber it with our stale snapshot. We only need to
            // carry `still_pending` forward in legacy mode (no registry to re-read).
            if st.session.is_none() {
                st.pending = still_pending;
            }

            // A resolved/errored asset resets the cadence so siblings stream in fast.
            if made_progress {
                st.delay_ms = MIN_DELAY_MS;
            }
            // Loop back: emit buffered events, or (if all still pending) tick again.
        }
    });

    // KeepAlive holds the connection through quiet stretches (e.g. a slow-to-process
    // asset) so intermediaries don't reap an idle stream; it emits a `: comment` line
    // every 15s, which EventSource silently ignores.
    let mut resp = Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(std::time::Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response();
    // Defense-in-depth for Nginx: disable response buffering on SSE streams.
    resp.headers_mut().insert(
        HeaderName::from_static("x-accel-buffering"),
        HeaderValue::from_static("no"),
    );
    resp.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-cache"),
    );
    resp.headers_mut()
        .insert(header::VARY, HeaderValue::from_static("Cookie"));
    resp
}

/// Finish beacon for the session-scoped SSE stream:
/// `POST /share/{key}/upload/finish?session=TOKEN`.
///
/// The client calls this once its upload loop ends. It validates that the caller is
/// authorized for upload on `{key}` and flips the session's `uploads_finished` flag
/// (scoped to `meta.canonical_key`) so the stream can terminate as soon as the
/// pending set drains, instead of waiting out the full wall-clock ceiling. Body-less;
/// returns 204. An unknown session token for that share is a harmless no-op (still
/// 204) — the stream's wall clock is the backstop either way. CSRF-checked like the
/// upload endpoint since it mutates session state.
pub async fn mark_upload_session_done_handler(
    headers: HeaderMap,
    Path(key): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    if !is_safe_param(&key) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if !check_csrf(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(tok) = params.get("session").filter(|t| is_safe_param(t)) else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    let client = ImmichClient::new();
    if client.upload_api_key.is_none() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let share_token = get_cookie_share_token(&headers, &key);
    let meta = match validate_upload_link(&client, &key, share_token.as_deref()).await {
        Ok(m) => m,
        Err(status) => return status.into_response(),
    };
    if !meta.is_album || !meta.allow_upload {
        return StatusCode::FORBIDDEN.into_response();
    }

    upload_sessions::mark_finished(&meta.canonical_key, tok);
    StatusCode::NO_CONTENT.into_response()
}

pub async fn upload_status_handler(
    headers: HeaderMap,
    Path((key, asset_id)): Path<(String, String)>,
) -> impl IntoResponse {
    // Back-compat single-asset endpoint. Prefer the batched `/share/{key}/status`
    // endpoint; this remains so older clients/bookmarks keep working. Shares the
    // same validation + resolution helpers (and their permission cache) as the
    // batch path, so it no longer re-implements the upstream calls inline.
    if !is_safe_param(&key) || !is_safe_param(&asset_id) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let client = ImmichClient::new();
    if client.upload_api_key.is_none() {
        return StatusCode::FORBIDDEN.into_response();
    }

    let share_token = get_cookie_share_token(&headers, &key);
    let meta = match validate_upload_link(&client, &key, share_token.as_deref()).await {
        Ok(m) => m,
        Err(status) => return status.into_response(),
    };
    if !meta.is_album || !meta.allow_upload {
        return StatusCode::FORBIDDEN.into_response();
    }

    let mut resp = match resolve_uploaded_asset(
        &client,
        &meta.canonical_key,
        &asset_id,
        meta.allow_download,
    )
    .await
    {
        // 202 Accepted: still processing tagging/association or thumbnail not ready.
        UploadedAssetStatus::Pending => StatusCode::ACCEPTED.into_response(),
        UploadedAssetStatus::Ready(asset) => (StatusCode::OK, axum::Json(*asset)).into_response(),
        UploadedAssetStatus::Error => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    insert_private_no_store(resp.headers_mut());
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::FromRequest;
    use axum::http::Request;

    #[test]
    fn test_upload_sessions_cross_share_isolation() {
        let sess = "test-sess-iso-1";
        upload_sessions::add_pending("share_a", sess, "asset_1");
        // Share B cannot inject into, read, finish, or delete Share A's session
        upload_sessions::add_pending("share_b", sess, "asset_evil");
        assert!(upload_sessions::snapshot_pending("share_b", sess).is_none());
        upload_sessions::mark_finished("share_b", sess);
        assert!(!upload_sessions::is_finished("share_a", sess));
        upload_sessions::remove_pending("share_b", sess, "asset_1");
        upload_sessions::remove_session("share_b", sess);

        // Share A retains sole ownership
        let snap = upload_sessions::snapshot_pending("share_a", sess).unwrap();
        assert_eq!(snap, vec!["asset_1".to_string()]);
        upload_sessions::mark_finished("share_a", sess);
        assert!(upload_sessions::is_finished("share_a", sess));
        upload_sessions::remove_session("share_a", sess);
        assert!(upload_sessions::snapshot_pending("share_a", sess).is_none());
    }

    #[test]
    fn test_processed_assets_cross_share_isolation() {
        record_processed_asset(
            "asset-iso-1".to_string(),
            "Alice".to_string(),
            "share_a".to_string(),
        );
        assert_eq!(
            lookup_processed_uploader("asset-iso-1", "share_b"),
            None
        );
        remove_processed_asset("asset-iso-1", "share_b");
        assert_eq!(
            lookup_processed_uploader("asset-iso-1", "share_a"),
            Some("Alice".to_string())
        );
        remove_processed_asset("asset-iso-1", "share_a");
        assert_eq!(lookup_processed_uploader("asset-iso-1", "share_a"), None);
    }

    #[test]
    fn test_media_security_header_helpers() {
        assert!(is_safe_inline_image_mime(Some("image/jpeg")));
        assert!(is_safe_inline_image_mime(Some("image/webp; charset=utf-8")));
        assert!(!is_safe_inline_image_mime(Some("image/svg+xml")));
        assert!(!is_safe_inline_image_mime(Some("text/html")));
        assert!(!is_safe_inline_image_mime(None));

        let inline = HeaderValue::from_static("inline; filename=\"pic.svg\"");
        assert_eq!(
            force_attachment_disposition(Some(&inline)),
            "attachment; filename=\"pic.svg\""
        );
        assert_eq!(force_attachment_disposition(None), "attachment");

        let pub_cc = HeaderValue::from_static("public, max-age=3600");
        assert_eq!(private_cache_control(Some(&pub_cc)), "private, max-age=3600");
    }

    async fn build_multipart(body: Vec<u8>, boundary: &str) -> Multipart {
        let req = Request::builder()
            .method("POST")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={}", boundary),
            )
            .body(Body::from(body))
            .unwrap();
        Multipart::from_request(req, &()).await.unwrap()
    }

    #[tokio::test]
    async fn test_streaming_multipart_filter_allows_valid_fields() {
        let b = "testbound";
        let raw = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"fileCreatedAt\"\r\n\r\n2026-01-01T00:00:00.000Z\r\n\
             --{b}\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"photo.jpg\"\r\nContent-Type: image/jpeg\r\n\r\nJPEGDATA\r\n\
             --{b}--\r\n"
        );
        let mp = build_multipart(raw.into_bytes(), b).await;
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let pump = tokio::spawn(async move {
            pump_filtered_multipart(mp, "outbound", 1024, ready_tx).await
        });
        let mut rx = ready_rx.await.unwrap().unwrap();
        let mut out = Vec::new();
        while let Some(item) = rx.recv().await {
            out.extend_from_slice(&item.unwrap());
        }
        assert_eq!(pump.await.unwrap(), Ok(()));
        let forwarded = String::from_utf8(out).unwrap();
        assert!(forwarded.contains("name=\"fileCreatedAt\""));
        assert!(forwarded.contains("name=\"assetData\"; filename=\"photo.jpg\""));
        assert!(forwarded.contains("JPEGDATA"));
        assert!(forwarded.ends_with("--outbound--\r\n"));
    }

    #[tokio::test]
    async fn test_streaming_multipart_filter_rejects_disallowed_field_before_upstream_connect() {
        let b = "testbound";
        let raw = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"isFavorite\"\r\n\r\ntrue\r\n\
             --{b}\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"photo.jpg\"\r\n\r\nDATA\r\n\
             --{b}--\r\n"
        );
        let mp = build_multipart(raw.into_bytes(), b).await;
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let pump = tokio::spawn(async move {
            pump_filtered_multipart(mp, "outbound", 1024, ready_tx).await
        });
        // Disallowed field is caught during pre-validation: ready_rx yields Err(400)
        // without ever creating an upstream stream!
        let ready_res = ready_rx.await.unwrap();
        assert_eq!(ready_res.unwrap_err(), StatusCode::BAD_REQUEST);
        assert_eq!(pump.await.unwrap(), Err(StatusCode::BAD_REQUEST));
    }

    #[tokio::test]
    async fn test_streaming_multipart_filter_rejects_oversized_upload() {
        let b = "testbound";
        let big = "X".repeat(2048);
        let raw = format!(
            "--{b}\r\nContent-Disposition: form-data; name=\"assetData\"; filename=\"big.bin\"\r\n\r\n{big}\r\n\
             --{b}--\r\n"
        );
        let mp = build_multipart(raw.into_bytes(), b).await;
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let pump = tokio::spawn(async move {
            pump_filtered_multipart(mp, "outbound", 512, ready_tx).await
        });
        if let Ok(Ok(mut rx)) = ready_rx.await {
            while rx.recv().await.is_some() {}
        }
        assert_eq!(pump.await.unwrap(), Err(StatusCode::PAYLOAD_TOO_LARGE));
    }
}
