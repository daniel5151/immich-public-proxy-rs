use crate::dto::SafeSharedLink;
use crate::immich_client::client::ImmichClient;
use crate::immich_client::model::Asset;
use crate::immich_client::model::SharedLink;
use axum::{
    Json,
    extract::{ConnectInfo, Path},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use ts_rs::TS;

#[derive(Debug, Clone, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "../frontend/src/types/generated/")]
pub struct ShareDetails {
    pub link: SafeSharedLink,
    pub password_required: bool,
    pub ipp_public_base_url: String,
    pub request_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareDetailsError {
    BadRequest,
    InvalidKey,
    NotAccessible,
    Internal,
}

/// TTL cache for fully-resolved share responses (IDEAS #4 + #6).
///
/// A cold `/share/{key}` build does `fetch_share_me` + `/albums/{id}` + per-uploader
/// tag searches + owner fallback. Caching the finished `ShareDetails` keyed by
/// `(share_key, Option<keyed_sha256(share_token)>)` collapses all of that to a single
/// clone on repeat loads, and—because uploader names are already stamped onto the
/// cached assets—also resolves the N+1 attribution work (#4).
///
/// Security & memory invariants:
/// - Keyed by `(String, Option<[u8; 32]>)` (plaintext share key + per-process keyed
///   SHA-256 digest of `share_token`) for 256-bit collision resistance, exact key
///   equality, and zero plaintext token retention.
/// - Public (non-password-protected) shares are stored under `(key, None)` and matched
///   first on `get()`, so arbitrary client cookies cannot bust or bloat the cache.
/// - Bounded to `MAX_CACHE_ENTRIES` (with expired + oldest eviction) and proactively
///   invalidated by the upload path whenever an album's contents change.
pub mod share_cache {
    use super::ShareDetails;
    use crate::security::hash_token_keyed;
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    const MAX_CACHE_ENTRIES: usize = 256;
    const MAX_GENERATION_ENTRIES: usize = 1024;

    type CacheKey = (String, Option<[u8; 32]>);

    // (share_key, Option<keyed_sha256(share_token)>) -> (details, inserted_at).
    // The plaintext share key is part of the map key so we can match and invalidate
    // every token variant of a key on mutation without needing to reverse the token
    // digest. The plaintext share_token itself is never stored.
    //
    // Lock ordering invariant: ALWAYS acquire CACHE before GENERATIONS when holding both.
    static CACHE: std::sync::OnceLock<
        parking_lot::RwLock<HashMap<CacheKey, (ShareDetails, Instant)>>,
    > = std::sync::OnceLock::new();

    // share_key -> monotonically increasing generation. Bumped on every invalidate so
    // an in-flight rebuild that began before a mutation can detect it became stale.
    static GENERATIONS: std::sync::OnceLock<parking_lot::RwLock<HashMap<String, u64>>> =
        std::sync::OnceLock::new();

    fn generations() -> &'static parking_lot::RwLock<HashMap<String, u64>> {
        GENERATIONS.get_or_init(|| parking_lot::RwLock::new(HashMap::new()))
    }

    fn ttl() -> Duration {
        static TTL: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
        *TTL.get_or_init(|| {
            let secs = std::env::var("IPP_TTL_SHARE_CACHE_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(45);
            Duration::from_secs(secs)
        })
    }

    fn token_digest(share_token: Option<&str>) -> Option<[u8; 32]> {
        share_token.map(hash_token_keyed)
    }

    /// Current generation for a key. Capture this *before* starting a rebuild and pass
    /// it back to `put` so a stale build (one that began before a concurrent mutation)
    /// can be rejected.
    pub fn generation(key: &str) -> u64 {
        generations().read().get(key).copied().unwrap_or(0)
    }

    pub fn get(key: &str, share_token: Option<&str>) -> Option<ShareDetails> {
        let t = ttl();
        if t.is_zero() {
            return None;
        }
        let cache = CACHE.get()?;
        let guard = cache.read();

        // Check public (token-less) entry first so arbitrary client cookies cannot
        // bust or bloat the cache for non-password-protected shares.
        if let Some((details, ts)) = guard.get(&(key.to_string(), None)) {
            if ts.elapsed() < t {
                return Some(details.clone());
            }
        }

        if let Some(digest) = token_digest(share_token) {
            if let Some((details, ts)) = guard.get(&(key.to_string(), Some(digest))) {
                if ts.elapsed() < t {
                    return Some(details.clone());
                }
            }
        }
        None
    }

    /// Store a freshly-built response, but only if no invalidation occurred for this
    /// key since `gen_at_start` was captured. Returns true if stored.
    ///
    /// Pass `share_token = None` for public shares (`link.password.is_none()`) so
    /// every visitor shares a single cached entry regardless of client cookies.
    pub fn put(
        key: &str,
        share_token: Option<&str>,
        details: &ShareDetails,
        gen_at_start: u64,
    ) -> bool {
        let t = ttl();
        if t.is_zero() {
            return false;
        }
        let cache = CACHE.get_or_init(|| parking_lot::RwLock::new(HashMap::new()));
        let cache_key = (key.to_string(), token_digest(share_token));
        let mut guard = cache.write();

        // Check generation while holding CACHE write lock (matches invalidate lock order).
        if generation(key) != gen_at_start {
            return false;
        }

        // Drop expired entries first, then enforce hard capacity cap.
        guard.retain(|_, (_, ts)| ts.elapsed() < t);
        if !guard.contains_key(&cache_key) {
            while guard.len() >= MAX_CACHE_ENTRIES {
                if let Some(oldest_key) = guard
                    .iter()
                    .min_by_key(|(_, (_, ts))| *ts)
                    .map(|(k, _)| k.clone())
                {
                    guard.remove(&oldest_key);
                } else {
                    break;
                }
            }
        }

        guard.insert(cache_key, (details.clone(), Instant::now()));
        true
    }

    /// Drop every cached variant of a share key and bump its generation (called when
    /// its album mutates). Holds `CACHE` write lock while bumping `GENERATIONS` so
    /// concurrent `put()` calls observe the bump atomically.
    pub fn invalidate(key: &str) {
        let cache = CACHE.get_or_init(|| parking_lot::RwLock::new(HashMap::new()));
        let mut keys_to_bump = std::collections::HashSet::new();
        keys_to_bump.insert(key.to_string());

        let mut guard = cache.write();
        guard.retain(|(k, _), (details, _)| {
            let matches = k == key || details.link.key == key || details.request_key == key;
            if matches {
                keys_to_bump.insert(k.clone());
                keys_to_bump.insert(details.link.key.clone());
                keys_to_bump.insert(details.request_key.clone());
                false
            } else {
                true
            }
        });

        let mut gens = generations().write();
        if gens.len() + keys_to_bump.len() > MAX_GENERATION_ENTRIES {
            // Keep generations map bounded; retain active cache keys and clear stale ones.
            let active_keys: std::collections::HashSet<&str> =
                guard.keys().map(|(k, _)| k.as_str()).collect();
            gens.retain(|k, _| active_keys.contains(k.as_str()) || keys_to_bump.contains(k));
        }
        for k in keys_to_bump {
            let g = gens.entry(k).or_insert(0);
            *g = g.wrapping_add(1);
        }
    }
}

/// Helper function to fetch share details. Used by the API handler and the HTML meta injector.
pub async fn get_share_details(
    key: String,
    headers: &HeaderMap,
) -> Result<ShareDetails, ShareDetailsError> {
    if !crate::security::is_safe_param(&key) {
        return Err(ShareDetailsError::BadRequest);
    }
    let client = ImmichClient::new();
    get_share_details_with_client(&client, key, headers).await
}

async fn get_share_details_with_client(
    client: &ImmichClient,
    key: String,
    headers: &HeaderMap,
) -> Result<ShareDetails, ShareDetailsError> {
    let ipp_public_base_url = crate::security::resolve_public_base_url(headers).map_err(|e| {
        eprintln!("get_share_details: invalid public base URL headers: {}", e);
        ShareDetailsError::BadRequest
    })?;

    // Read share token exclusively from the proxy's validated HttpOnly cookie.
    let share_token = crate::immich_client::client::get_cookie_share_token(headers, &key);

    // Serve a fresh cached response when one is available (IDEAS #4 + #6).
    if let Some(mut cached) = share_cache::get(&key, share_token.as_deref()) {
        // The cached body is request-independent except for these two fields, which
        // derive from the current request's host header / path; re-stamp them.
        cached.ipp_public_base_url = ipp_public_base_url;
        cached.request_key = key;
        return Ok(cached);
    }

    // Snapshot the cache generation *before* building. If a mutation invalidates this
    // key while we build, `put` below detects the generation bump and refuses to store
    // our now-stale result (prevents a read-after-write stampede).
    let cache_generation = share_cache::generation(&key);

    let (status, text) = client
        .fetch_share_me(&key, share_token.as_deref())
        .await
        .map_err(|e| {
            eprintln!("fetch_share_me transport error for key {:?}: {}", key, e);
            ShareDetailsError::Internal
        })?;

    if status == 401 {
        return server_helpers::handle_unauthorized(client, &key, &text, ipp_public_base_url).await;
    }

    if !status.is_success() {
        eprintln!(
            "fetch_share_me failed for key {:?}: {} — {}",
            key, status, text
        );
        return Err(ShareDetailsError::Internal);
    }

    let link: SharedLink = serde_json::from_str(&text).map_err(|e| {
        eprintln!(
            "fetch_share_me JSON parse error for key {:?}: {}",
            key, e
        );
        ShareDetailsError::Internal
    })?;
    let canonical_key = link.key.clone();
    let is_password_protected = link.password.is_some();

    let allow_download = link.allow_download.unwrap_or(false);
    let show_metadata = link.show_metadata.unwrap_or(true);
    let mut safe_link = SafeSharedLink::from_base(link.clone());

    // Immich v3: SharedLinkResponseDto no longer includes album assets inline.
    // For album-type shares, fetch assets via the v3 timeline bucket endpoints.
    if link.r#type.as_deref() == Some("ALBUM") {
        if let Some(ref album) = link.album {
            if safe_link.assets.is_empty() {
                let assets = client
                    .fetch_album_assets(&album.id, &canonical_key, share_token.as_deref())
                    .await
                    .map_err(|e| {
                        eprintln!(
                            "Failed to fetch album assets for key {:?}: {}",
                            key, e
                        );
                        ShareDetailsError::Internal
                    })?;
                safe_link.assets = assets
                    .into_iter()
                    .map(crate::dto::SafeAsset::from_base)
                    .collect();
            }
            if show_metadata {
                server_helpers::resolve_uploader_names(client, &album.id, &mut safe_link.assets)
                    .await;
            }
        }
    } else if show_metadata {
        server_helpers::resolve_owner_fallback(client, &mut safe_link.assets).await;
    }

    // Sort album assets if there is a sort order specified
    if let Some(ref album) = link.album {
        match album.order.as_deref() {
            Some("asc") => safe_link
                .assets
                .sort_by(|a, b| a.file_created_at.cmp(&b.file_created_at)),
            Some("desc") => safe_link
                .assets
                .sort_by(|a, b| b.file_created_at.cmp(&a.file_created_at)),
            _ => {}
        }
    }

    // If all assets have the exact same uploader, omit the badges
    server_helpers::strip_uniform_uploader(&mut safe_link);

    // Stamp download_url server-side when downloads are allowed
    if allow_download {
        for asset in &mut safe_link.assets {
            asset.download_url = Some(format!(
                "/share/photo/{}/{}/original",
                safe_link.key, asset.id
            ));
        }
    }

    let details = ShareDetails {
        link: safe_link,
        password_required: false,
        ipp_public_base_url,
        request_key: key.clone(),
    };

    // For public shares (no password), cache under `(key, None)` so arbitrary client
    // cookies share the same entry and cannot cause cache bloat or cache-busting.
    let cache_token = if is_password_protected {
        share_token.as_deref()
    } else {
        None
    };
    share_cache::put(&key, cache_token, &details, cache_generation);

    Ok(details)
}

/// Axum JSON API handler
pub async fn get_share_details_handler(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(key): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !crate::security::is_safe_param(&key) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid share key" })),
        )
            .into_response();
    }

    let client_ip = crate::security::extract_client_ip(&headers, peer.ip());
    if let Err(retry_after) = crate::security::probe_limiter().check_blocked(client_ip) {
        return crate::security::rate_limited_response(
            retry_after,
            Json(serde_json::json!({ "error": "Too many requests" })),
        );
    }

    let mut resp = match get_share_details(key, &headers).await {
        Ok(details) => Json(details).into_response(),
        Err(ShareDetailsError::BadRequest) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Bad request" })),
        )
            .into_response(),
        Err(ShareDetailsError::InvalidKey) => {
            crate::security::probe_limiter().penalize(client_ip);
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "Invalid share key" })),
            )
                .into_response()
        }
        Err(ShareDetailsError::NotAccessible) => {
            crate::security::probe_limiter().penalize(client_ip);
            (
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({ "error": "Share is not accessible" })),
            )
                .into_response()
        }
        Err(ShareDetailsError::Internal) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "Internal server error" })),
        )
            .into_response(),
    };

    crate::security::insert_private_no_store(resp.headers_mut());
    resp
}

mod server_helpers {
    use super::*;

    /// Handles a 401 response by consulting the admin API to determine
    /// whether the share requires a password or is simply invalid.
    pub async fn handle_unauthorized(
        client: &ImmichClient,
        key: &str,
        body: &str,
        ipp_public_base_url: String,
    ) -> Result<ShareDetails, ShareDetailsError> {
        // Immich answers a password-protected share with HTTP 401 and a body of
        // `{"message":"Password required",...}` (no password supplied) or
        // `{"message":"Invalid password",...}` (wrong password). In both cases
        // the right response is the password prompt — "Invalid password" means
        // try again, not "bad share". Trust the body directly: it works
        // regardless of which user owns the link, whereas the admin
        // `/shared-links` lookup below only sees links owned by the admin API
        // key's user and would otherwise misreport someone else's
        // password-protected share as an invalid key.
        if body.contains("Password required") || body.contains("Invalid password") {
            eprintln!("Share {:?} requires a password (per Immich 401 body)", key);
            return Ok(password_required_response(key, ipp_public_base_url));
        }

        match client.get_admin_shared_link(key).await {
            Ok(Some(link)) if link.password.is_some() => {
                eprintln!("Share {:?} requires a password", key);
                Ok(password_required_response(key, ipp_public_base_url))
            }
            Ok(Some(_)) => {
                eprintln!(
                    "Share {:?} returned 401 but has no password set — share is not accessible",
                    key
                );
                Err(ShareDetailsError::NotAccessible)
            }
            Ok(None) if client.admin_api_key.is_some() => {
                // Admin API is available but the link wasn't found
                eprintln!(
                    "Share {:?} not found via admin API — invalid share key",
                    key
                );
                Err(ShareDetailsError::InvalidKey)
            }
            _ => {
                // No admin key or API error — can't determine the cause,
                // so assume password required as a safe fallback
                eprintln!(
                    "Share {:?} returned 401 and admin API is unavailable — assuming password required",
                    key
                );
                Ok(password_required_response(key, ipp_public_base_url))
            }
        }
    }

    /// Builds a `ShareDetails` indicating a password is required.
    fn password_required_response(key: &str, ipp_public_base_url: String) -> ShareDetails {
        ShareDetails {
            link: SafeSharedLink {
                key: key.to_string(),
                description: None,
                r#type: None,
                allow_download: None,
                allow_upload: None,
                assets: vec![],
                album: None,
            },
            password_required: true,
            ipp_public_base_url,
            request_key: key.to_string(),
        }
    }

    /// If every asset has the exact same uploader name, clear all badges
    /// (since a uniform badge adds no information).
    pub fn strip_uniform_uploader(link: &mut SafeSharedLink) {
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
        assets: &mut [crate::dto::SafeAsset],
    ) {
        resolve_shared_by_tags(client, album_id, assets).await;
        resolve_owner_fallback(client, assets).await;
    }

    /// Looks up `SharedBy/{name}` tags via the admin API or the upload service account API and stamps matching
    /// assets with the tag name as `uploader_name`.
    async fn resolve_shared_by_tags(
        client: &ImmichClient,
        album_id: &str,
        assets: &mut [crate::dto::SafeAsset],
    ) {
        if let Some(ref upload_key) = client.upload_api_key {
            resolve_shared_by_tags_for_key(client, upload_key, album_id, assets).await;
        }
        if let Some(ref admin_key) = client.admin_api_key {
            if Some(admin_key) != client.upload_api_key.as_ref() {
                resolve_shared_by_tags_for_key(client, admin_key, album_id, assets).await;
            }
        }
    }

    async fn resolve_shared_by_tags_for_key(
        client: &ImmichClient,
        api_key: &str,
        album_id: &str,
        assets: &mut [crate::dto::SafeAsset],
    ) {
        let Some(tags_res): Option<reqwest::Response> = client.get_with_key("/tags", api_key).await
        else {
            return;
        };
        if !tags_res.status().is_success() {
            static WARN_ONCE: std::sync::Once = std::sync::Once::new();
            WARN_ONCE.call_once(|| {
                eprintln!(
                    "warning: API /tags failed: {} — uploader attribution via SharedBy/ tags will be unavailable",
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

                let Some(search_res): Option<reqwest::Response> = client
                    .post_with_key("/search/metadata", api_key, &search_req)
                    .await
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
    pub async fn resolve_owner_fallback(
        client: &ImmichClient,
        assets: &mut [crate::dto::SafeAsset],
    ) {
        let mut users_map = std::collections::HashMap::new();
        let mut users_fetched = false;

        if assets.iter().any(|a| a.uploader_name.is_none()) {
            if let Some(res) = client.admin_get("/users").await {
                if res.status().is_success() {
                    if let Ok(users) = res.json::<Vec<crate::immich_client::model::User>>().await {
                        for u in users {
                            users_map.insert(u.id, u.name);
                        }
                        users_fetched = true;
                    }
                } else {
                    static WARN_ONCE: std::sync::Once = std::sync::Once::new();
                    WARN_ONCE.call_once(|| {
                        eprintln!(
                            "warning: Admin API /users failed: {} — falling back to sequential /assets/{{id}} requests",
                            res.status()
                        );
                    });
                }
            }
        }

        for asset in assets.iter_mut() {
            if asset.uploader_name.is_some() {
                continue;
            }

            if users_fetched {
                if let Some(ref owner_id) = asset.owner_id {
                    if let Some(name) = users_map.get(owner_id) {
                        asset.uploader_name = Some(name.clone());
                        asset.uploader_is_fallback = true;
                        continue;
                    }
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_details(key: &str) -> ShareDetails {
        ShareDetails {
            link: SafeSharedLink {
                key: key.to_string(),
                description: Some(format!("Album {}", key)),
                r#type: Some("ALBUM".to_string()),
                allow_download: Some(true),
                allow_upload: Some(false),
                assets: vec![],
                album: None,
            },
            password_required: false,
            ipp_public_base_url: "https://example.com".to_string(),
            request_key: key.to_string(),
        }
    }

    #[test]
    fn test_share_cache_token_isolation_and_public_collapsing() {
        let priv_key = "test_priv_key_iso";
        let g = share_cache::generation(priv_key);
        let d = sample_details(priv_key);
        assert!(share_cache::put(priv_key, Some("tok_a"), &d, g));
        assert!(share_cache::get(priv_key, Some("tok_a")).is_some());
        assert!(share_cache::get(priv_key, Some("tok_b")).is_none());
        assert!(share_cache::get(priv_key, None).is_none());

        // Public share stored with None is served even if client sends arbitrary cookie
        let pub_key = "test_pub_key_collapse";
        let gp = share_cache::generation(pub_key);
        let dp = sample_details(pub_key);
        assert!(share_cache::put(pub_key, None, &dp, gp));
        assert!(share_cache::get(pub_key, None).is_some());
        assert!(share_cache::get(pub_key, Some("random_junk")).is_some());

        // Invalidation bumps generation and rejects stale writes
        share_cache::invalidate(pub_key);
        assert!(share_cache::get(pub_key, None).is_none());
        assert!(!share_cache::put(pub_key, None, &dp, gp));
    }
}
