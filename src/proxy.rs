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
use axum::response::sse::Event;
use axum::response::sse::KeepAlive;
use axum::response::sse::Sse;
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
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '_'
            }
        })
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
    parking_lot::RwLock<std::collections::HashMap<String, (String, std::time::Instant)>>,
> = std::sync::OnceLock::new();

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
// The session token is minted by the client (crypto.randomUUID) and passed as the
// `session` query param on each upload POST and on the stream open. It's validated
// with the same `is_safe_param` rule as every other path segment (a UUID is
// alphanumeric + '-', which that rule already permits).
mod upload_sessions {
    use std::collections::{HashMap, HashSet};
    use std::time::{Duration, Instant};

    struct Session {
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

    /// Register a freshly-uploaded asset id under `session`, creating the session on
    /// first use. Also lazily sweeps sessions older than the TTL (same cheap
    /// retain-on-write pattern as `status_link_cache::put`) so abandoned ones can't
    /// leak.
    pub(super) fn add_pending(session: &str, asset_id: &str) {
        let mut guard = map().write();
        let now = Instant::now();
        guard.retain(|_, s| now.duration_since(s.created) < SESSION_TTL);
        let entry = guard.entry(session.to_string()).or_insert_with(|| Session {
            pending: HashSet::new(),
            uploads_finished: false,
            created: now,
        });
        entry.pending.insert(asset_id.to_string());
    }

    /// Mark a session's uploads complete. No-op if the session is unknown (e.g. the
    /// finish beacon raced ahead of registration — unlikely, but harmless).
    pub(super) fn mark_finished(session: &str) {
        if let Some(s) = map().write().get_mut(session) {
            s.uploads_finished = true;
        }
    }

    /// Snapshot the session's current pending ids. `None` if the session is unknown
    /// (never created, or already cleaned up).
    pub(super) fn snapshot_pending(session: &str) -> Option<Vec<String>> {
        map()
            .read()
            .get(session)
            .map(|s| s.pending.iter().cloned().collect())
    }

    /// True once the client has signaled no more uploads are coming. Unknown sessions
    /// report false (keep waiting until the stream's wall-clock guard fires).
    pub(super) fn is_finished(session: &str) -> bool {
        map()
            .read()
            .get(session)
            .map(|s| s.uploads_finished)
            .unwrap_or(false)
    }

    /// Drop an id from the session's pending set once the stream has resolved it.
    pub(super) fn remove_pending(session: &str, asset_id: &str) {
        if let Some(s) = map().write().get_mut(session) {
            s.pending.remove(asset_id);
        }
    }

    /// Remove the whole session (called when its stream terminates).
    pub(super) fn remove_session(session: &str) {
        map().write().remove(session);
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
        .header("x-api-key", upload_key)
        .json(&create_body)
        .send()
        .await;

    match create_res {
        Ok(res) if res.status().is_success() || res.status() == StatusCode::CREATED => {
            if let Ok(created_tag) = res.json::<crate::immich_client::model::Tag>().await {
                let mut write_guard = cache.write();
                write_guard.insert(cache_key.clone(), created_tag.id.clone());
                return Some(created_tag.id);
            }
            eprintln!(
                "upload: tag create for '{}' returned success but body failed to parse; re-reading /tags",
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
                "upload: tag create for '{}' returned status {} (likely already exists) — recovering via /tags re-read; body: {}",
                cache_key,
                status,
                body.chars().take(200).collect::<String>()
            );
        }
        Err(e) => {
            eprintln!(
                "upload: tag create request for '{}' failed at transport level: {} — recovering via /tags re-read",
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
        "upload: FAILED to get-or-create tag '{}' after create + 3 recovery reads",
        cache_key
    );
    None
}

/// GET /tags, bulk-cache every tag by (parent,name) key, and return the id for
/// `cache_key` if present. Returns None on transport/parse failure or cache miss.
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
    for tag in &tags {
        let key = match &tag.parent_id {
            Some(p_id) => format!("{}:{}", p_id, tag.name),
            None => format!("root:{}", tag.name),
        };
        // Overwrite: /tags is authoritative, so a fresh listing should win over any
        // stale cache entry.
        write_guard.insert(key, tag.id.clone());
    }
    write_guard.get(cache_key).cloned()
}

static IMMICH_API_SEMAPHORE: std::sync::OnceLock<tokio::sync::Semaphore> =
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
    // path already handles concurrent creation via a POST-conflict re-query.
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
                        "upload: could not resolve child tag '{}' for asset {} (outer attempt {})",
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
async fn deferred_tag_guard(client: &ImmichClient, asset_id: &str, uploader_name: &str, key: &str) {
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

    // Re-resolve the child tag id (cached after the synchronous tagging path ran).
    let parent_tag_id = get_or_create_tag(client, "SharedBy", None).await;
    let child_tag_id = match &parent_tag_id {
        Some(p) => get_or_create_tag(client, uploader_name, Some(p)).await,
        None => None,
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

        match asset_tag_state(client, asset_id, &child_tag_id).await {
            TagState::Present => {
                consecutive_present += 1;
                // Two spaced confirmations => extraction has run and link is stable.
                if consecutive_present >= 2 {
                    if restored {
                        // Attribution was genuinely restored after a wipe; refresh
                        // the share cache so viewers see the corrected uploader.
                        crate::api::get_share_details::share_cache::invalidate(key);
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
                // Confirmed absent — Immich wiped it (or it never settled). Re-apply.
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
    match asset_tag_state(client, asset_id, &child_tag_id).await {
        TagState::Present => {
            if restored {
                crate::api::get_share_details::share_cache::invalidate(key);
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

pub async fn upload_asset_handler(
    headers: HeaderMap,
    Path(key): Path<String>,
    // Optional `?session=TOKEN` ties this upload to a session-scoped SSE stream (see
    // `mod upload_sessions`). `Query` reads only the URI, so it's safe to extract
    // before the body-consuming `Request`.
    Query(params): Query<std::collections::HashMap<String, String>>,
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
        .and_then(|a| a.owner.as_ref())
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
                "upload: failed to parse upload response for key '{}': {}",
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
        "upload: asset {} accepted (status={:?}, uploader={:?}, key={})",
        asset_id, upload_resp.status, uploader_name, key
    );

    // Session-scoped SSE support: if the client supplied an upload session token,
    // register this asset id under it so an already-open status stream picks it up on
    // its next tick (see `mod upload_sessions`). This is what lets photos stream back
    // while later files in the same drop are still uploading. The token is validated
    // with the same is_safe_param rule as every path segment; an absent/invalid token
    // is a no-op, so the poll path and the legacy ids= stream are unaffected.
    if let Some(session) = params.get("session") {
        if !session.is_empty() && is_safe_param(session) {
            upload_sessions::add_pending(session, &asset_id);
        }
    }

    // Album contents just changed; drop any cached share response so the next load
    // rebuilds (IDEAS #6 invalidation). Re-invalidated below once tagging/association
    // completes, since that mutates the asset's attribution too.
    crate::api::get_share_details::share_cache::invalidate(&key);

    // Spawn background task to tag and associate the asset, saving it to PROCESSED_ASSETS when done.
    let client_clone = client.clone();
    let asset_id_clone = asset_id.clone();
    let album_id_clone = album_id.clone();
    let uploader_name_clone = uploader_name.clone();
    let key_clone = key.clone();

    tokio::spawn(async move {
        let success = tag_and_associate_asset(
            &client_clone,
            &asset_id_clone,
            &album_id_clone,
            &uploader_name_clone,
        )
        .await;

        // Attribution/album membership settled — invalidate again so the resolved
        // uploader name is reflected on the next share load.
        crate::api::get_share_details::share_cache::invalidate(&key_clone);

        if success {
            let cache = PROCESSED_ASSETS
                .get_or_init(|| parking_lot::RwLock::new(std::collections::HashMap::new()));
            let mut write_guard = cache.write();

            // Clean up stale entries to prevent memory leaks from abandoned status polls
            let now = std::time::Instant::now();
            let expiry = std::time::Duration::from_secs(600); // 10 minutes
            write_guard.retain(|_, (_, timestamp)| now.duration_since(*timestamp) < expiry);

            write_guard.insert(asset_id_clone.clone(), (uploader_name_clone.clone(), now));
        }

        // Deferred guard: Immich's async metadata-extraction job can REPLACE this
        // asset's tag set (with the empty set parsed from a keyword-less file) AFTER
        // the synchronous PUT+verify above already confirmed the tag, silently
        // orphaning it. Run a detached guard that re-checks across the extraction
        // window and re-applies the tag if it vanishes. Only runs once the initial
        // tag/associate succeeded, so it defends a known-good attribution.
        if success {
            let guard_client = client_clone.clone();
            let guard_asset = asset_id_clone.clone();
            let guard_uploader = uploader_name_clone.clone();
            let guard_key = key_clone.clone();
            tokio::spawn(async move {
                deferred_tag_guard(&guard_client, &guard_asset, &guard_uploader, &guard_key).await;
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
// Upload-status validation + resolution helpers (shared by the single-asset and
// batched status endpoints). See IDEAS.local.md "avoid slamming the status
// endpoint": a big album drop used to spawn one 500ms poll loop *per asset*, and
// every poll re-validated the share key via `fetch_share_me` (an upstream hit) on
// top of the thumbnail/asset reads. That amplified N uploads into ~3N upstream
// calls/sec. These helpers let the batched endpoint validate once per request and
// reuse a short-TTL permission cache across polls, collapsing the storm.
// ---------------------------------------------------------------------------

/// Immutable permission facts about a share link, as far as the upload-status
/// path cares. All `Copy`, so the cache hands back values without cloning.
#[derive(Clone, Copy)]
struct UploadLinkMeta {
    is_album: bool,
    allow_upload: bool,
    allow_download: bool,
}

/// Short-TTL cache of share-link *permission* metadata, keyed by (key, password).
///
/// Deliberately separate from `share_cache`: that one caches album *contents* and
/// is invalidated on every upload, so it would miss constantly during an active
/// drop. A link's *permissions* (type / allow_upload / allow_download) don't change
/// when its album contents do, so a stale-but-valid read within the TTL is safe and
/// lets a burst of status polls skip re-hitting Immich's `/shared-links/me`.
mod status_link_cache {
    use super::UploadLinkMeta;
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    use std::time::{Duration, Instant};

    static CACHE: std::sync::OnceLock<
        parking_lot::RwLock<HashMap<u64, (UploadLinkMeta, Instant)>>,
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

    fn entry_hash(key: &str, password: Option<&str>) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut h);
        match password {
            Some(p) => {
                1u8.hash(&mut h);
                p.hash(&mut h);
            }
            None => 0u8.hash(&mut h),
        }
        h.finish()
    }

    pub(super) fn get(key: &str, password: Option<&str>) -> Option<UploadLinkMeta> {
        let t = ttl();
        if t.is_zero() {
            return None;
        }
        let cache = CACHE.get()?;
        let guard = cache.read();
        let (meta, ts) = guard.get(&entry_hash(key, password))?;
        if ts.elapsed() < t { Some(*meta) } else { None }
    }

    pub(super) fn put(key: &str, password: Option<&str>, meta: UploadLinkMeta) {
        let t = ttl();
        if t.is_zero() {
            return;
        }
        let cache = CACHE.get_or_init(|| parking_lot::RwLock::new(HashMap::new()));
        let mut guard = cache.write();
        guard.retain(|_, (_, ts)| ts.elapsed() < t);
        guard.insert(entry_hash(key, password), (meta, Instant::now()));
    }
}

/// Validate that `key` is an upload-enabled album share, reusing a short-TTL
/// permission cache so repeated status polls don't each hit Immich. On a cache
/// miss this calls `fetch_share_me` once and memoizes the result.
///
/// `Ok(meta)` means the key resolved; the caller still checks `is_album` /
/// `allow_upload`. `Err(status)` is an upstream/parse failure to surface verbatim.
async fn validate_upload_link(
    client: &ImmichClient,
    key: &str,
    password: Option<&str>,
) -> Result<UploadLinkMeta, StatusCode> {
    if let Some(meta) = status_link_cache::get(key, password) {
        return Ok(meta);
    }

    let share_link = match client.fetch_share_me(key, password).await {
        Ok((status, text)) if status.is_success() => {
            match serde_json::from_str::<crate::immich_client::model::SharedLink>(&text) {
                Ok(link) => link,
                Err(e) => {
                    eprintln!("status: failed to parse share link response: {}", e);
                    return Err(StatusCode::INTERNAL_SERVER_ERROR);
                }
            }
        }
        Ok((status, _)) => {
            return Err(
                StatusCode::from_u16(status.as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
            );
        }
        Err(e) => {
            eprintln!("status: failed to fetch share link: {}", e);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let meta = UploadLinkMeta {
        is_album: share_link.r#type.as_deref() == Some("ALBUM"),
        allow_upload: share_link.allow_upload.unwrap_or(false),
        allow_download: share_link.allow_download.unwrap_or(false),
    };
    status_link_cache::put(key, password, meta);
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

/// Resolve one asset by id: pending until it's in `PROCESSED_ASSETS` *and* Immich
/// has generated its thumbnail, then fetch the full asset and build a `SafeAsset`.
/// On `Ready` the asset is removed from `PROCESSED_ASSETS`. No upstream calls are
/// made for a still-pending asset (the in-memory map check short-circuits first),
/// so polling a not-yet-ready batch costs nothing upstream.
async fn resolve_uploaded_asset(
    client: &ImmichClient,
    key: &str,
    asset_id: &str,
    allow_download: bool,
) -> UploadedAssetStatus {
    let uploader_name = {
        let cache = PROCESSED_ASSETS
            .get_or_init(|| parking_lot::RwLock::new(std::collections::HashMap::new()));
        let read_guard = cache.read();
        match read_guard.get(asset_id) {
            Some((name, _)) => name.clone(),
            None => return UploadedAssetStatus::Pending,
        }
    };

    // Thumbnail generated yet?
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
        _ => return UploadedAssetStatus::Pending,
    }

    // Fetch final asset info to build a complete SafeAsset.
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
        safe_asset.download_url = Some(format!("/share/photo/{}/{}/original", key, safe_asset.id));
    }

    // Resolved — drop it from the pending map.
    {
        let cache = PROCESSED_ASSETS
            .get_or_init(|| parking_lot::RwLock::new(std::collections::HashMap::new()));
        cache.write().remove(asset_id);
    }

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

    let cookie_password = get_cookie_password(&headers, &key);
    let meta = match validate_upload_link(&client, &key, cookie_password.as_deref()).await {
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
        match resolve_uploaded_asset(&client, &key, id, meta.allow_download).await {
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
    (
        StatusCode::OK,
        axum::Json(BatchStatusResponse {
            ready,
            pending,
            errored,
        }),
    )
        .into_response()
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
/// the 256-id `MAX_BATCH` cap on the legacy ids path, and a single
/// `validate_upload_link` call (which reuses the short-TTL permission cache), so the
/// share key is validated ONCE for the life of the stream rather than once per poll.
/// Inside the stream we reuse `resolve_uploaded_asset` for every id — no
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
    let cookie_password = get_cookie_password(&headers, &key);
    let meta = match validate_upload_link(&client, &key, cookie_password.as_deref()).await {
        Ok(m) => m,
        Err(status) => return status.into_response(),
    };
    if !meta.is_album || !meta.allow_upload {
        return StatusCode::FORBIDDEN.into_response();
    }

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
        key: String,
        allow_download: bool,
        session: Option<String>,
        pending: Vec<String>,
        resolved: usize,
        out: std::collections::VecDeque<Event>,
        start: std::time::Instant,
        delay_ms: u64,
        finished: bool,
    }

    // Same cadence spirit as the poll loop: 500ms -> 1s -> 2s (capped). A freshly
    // resolved asset resets the delay so its siblings stream in quickly.
    const MIN_DELAY_MS: u64 = 500;
    const MAX_DELAY_MS: u64 = 2000;
    const MAX_WALL_MS: u128 = 120_000;

    let init = StreamState {
        client,
        key,
        allow_download: meta.allow_download,
        session,
        pending: legacy_ids, // empty in session mode; the registry is the source of truth there
        resolved: 0,
        out: std::collections::VecDeque::new(),
        start: std::time::Instant::now(),
        delay_ms: MIN_DELAY_MS,
        finished: false,
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
                    upload_sessions::remove_session(sess);
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
                    if let Some(live) = upload_sessions::snapshot_pending(sess) {
                        st.pending = live;
                    }
                    upload_sessions::is_finished(sess)
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
                match resolve_uploaded_asset(&st.client, &st.key, &id, st.allow_download).await {
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
                            upload_sessions::remove_pending(sess, &id);
                        }
                        st.resolved += 1;
                        made_progress = true;
                    }
                    UploadedAssetStatus::Error => {
                        st.out
                            .push_back(Event::default().event("errored").data(id.clone()));
                        if let Some(ref sess) = st.session {
                            upload_sessions::remove_pending(sess, &id);
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
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(std::time::Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

/// Finish beacon for the session-scoped SSE stream:
/// `POST /share/{key}/upload/finish?session=TOKEN`.
///
/// The client calls this once its upload loop ends. It flips the session's
/// `uploads_finished` flag so the stream can terminate as soon as the pending set
/// drains, instead of waiting out the full wall-clock ceiling. Body-less; returns
/// 204. An unknown/invalid token is a harmless no-op (still 204) — the stream's
/// wall clock is the backstop either way. CSRF-checked like the upload endpoint
/// since it mutates session state.
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
    match params.get("session") {
        Some(tok) if !tok.is_empty() && is_safe_param(tok) => {
            upload_sessions::mark_finished(tok);
            StatusCode::NO_CONTENT.into_response()
        }
        _ => StatusCode::BAD_REQUEST.into_response(),
    }
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

    let cookie_password = get_cookie_password(&headers, &key);
    let meta = match validate_upload_link(&client, &key, cookie_password.as_deref()).await {
        Ok(m) => m,
        Err(status) => return status.into_response(),
    };
    if !meta.is_album || !meta.allow_upload {
        return StatusCode::FORBIDDEN.into_response();
    }

    match resolve_uploaded_asset(&client, &key, &asset_id, meta.allow_download).await {
        // 202 Accepted: still processing tagging/association or thumbnail not ready.
        UploadedAssetStatus::Pending => StatusCode::ACCEPTED.into_response(),
        UploadedAssetStatus::Ready(asset) => (StatusCode::OK, axum::Json(*asset)).into_response(),
        UploadedAssetStatus::Error => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
