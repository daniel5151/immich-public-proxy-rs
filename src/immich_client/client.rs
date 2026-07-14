use reqwest::Client;
use reqwest::Url;

#[derive(Clone)]
pub struct ImmichClient {
    pub api_url: String,
    pub http_client: Client,
    pub admin_api_key: Option<String>,
    pub upload_api_key: Option<String>,
}

impl ImmichClient {
    pub fn new() -> Self {
        static API_URL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        let api_url = API_URL
            .get_or_init(|| {
                std::env::var("IMMICH_URL")
                    .expect("IMMICH_URL environment variable must be set")
                    .trim_end_matches('/')
                    .to_string()
                    + "/api"
            })
            .clone();

        static CLIENT: std::sync::OnceLock<Client> = std::sync::OnceLock::new();
        let http_client = CLIENT
            .get_or_init(|| {
                Client::builder()
                    .connect_timeout(std::time::Duration::from_secs(10))
                    .build()
                    .expect("Failed to build HTTP client")
            })
            .clone();

        static ADMIN_API_KEY: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        let admin_api_key = ADMIN_API_KEY
            .get_or_init(|| std::env::var("IMMICH_API_KEY").ok())
            .clone();

        static UPLOAD_API_KEY: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        let upload_api_key = UPLOAD_API_KEY
            .get_or_init(|| std::env::var("IMMICH_API_KEY_UPLOAD_USER").ok())
            .clone();

        Self {
            api_url,
            http_client,
            admin_api_key,
            upload_api_key,
        }
    }

    /// Fetches the user ID associated with the upload API key, caching the result thread-safely.
    pub async fn get_upload_user_id(&self) -> Option<String> {
        static UPLOAD_USER_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        if let Some(cached) = UPLOAD_USER_ID.get() {
            return Some(cached.clone());
        }

        let upload_key = self.upload_api_key.as_ref()?;
        let url = self.build_url("/users/me", &[]);
        let res = self
            .http_client
            .get(&url)
            .header("x-api-key", upload_key)
            .send()
            .await
            .ok()?;

        if res.status().is_success() {
            if let Ok(user) = res.json::<crate::immich_client::model::User>().await {
                let user_id = user.id;
                UPLOAD_USER_ID.get_or_init(|| user_id.clone());
                return Some(user_id);
            }
        }

        None
    }

    pub fn build_url(&self, path: &str, params: &[(&str, &str)]) -> String {
        let url = format!("{}{}", self.api_url, path);
        let mut u = Url::parse(&url).unwrap();
        if !params.is_empty() {
            u.query_pairs_mut()
                .extend_pairs(params.iter().filter(|(_, v)| !v.is_empty()));
        }
        u.to_string()
    }

    /// Sends an authenticated GET request using the specified API key.
    pub async fn get_with_key(&self, path: &str, key: &str) -> Option<reqwest::Response> {
        let url = self.build_url(path, &[]);
        self.http_client
            .get(&url)
            .header("x-api-key", key)
            .send()
            .await
            .ok()
    }

    /// Sends an authenticated POST request using the specified API key.
    pub async fn post_with_key(
        &self,
        path: &str,
        key: &str,
        body: &impl serde::Serialize,
    ) -> Option<reqwest::Response> {
        let url = self.build_url(path, &[]);
        self.http_client
            .post(&url)
            .header("x-api-key", key)
            .json(body)
            .send()
            .await
            .ok()
    }

    /// Sends an authenticated PUT request using the specified API key.
    pub async fn put_with_key(
        &self,
        path: &str,
        key: &str,
        body: &impl serde::Serialize,
    ) -> Option<reqwest::Response> {
        let url = self.build_url(path, &[]);
        self.http_client
            .put(&url)
            .header("x-api-key", key)
            .json(body)
            .send()
            .await
            .ok()
    }

    /// Sends an authenticated GET request using the admin API key.
    /// Returns `None` if there is no admin API key configured.
    pub async fn admin_get(&self, path: &str) -> Option<reqwest::Response> {
        let admin_key = self.admin_api_key.as_ref()?;
        self.get_with_key(path, admin_key).await
    }

    /// Sends an authenticated POST request using the admin API key.
    /// Returns `None` if there is no admin API key configured.
    #[allow(dead_code)]
    pub async fn admin_post(
        &self,
        path: &str,
        body: &impl serde::Serialize,
    ) -> Option<reqwest::Response> {
        let admin_key = self.admin_api_key.as_ref()?;
        self.post_with_key(path, admin_key, body).await
    }

    /// Sends an authenticated PUT request using the admin API key.
    /// Returns `None` if there is no admin API key configured.
    pub async fn admin_put(
        &self,
        path: &str,
        body: &impl serde::Serialize,
    ) -> Option<reqwest::Response> {
        let admin_key = self.admin_api_key.as_ref()?;
        self.put_with_key(path, admin_key, body).await
    }
    /// Queries the admin `/shared-links` endpoint to find a link by its key or slug.
    pub async fn get_admin_shared_link(
        &self,
        key_or_slug: &str,
    ) -> Result<Option<crate::immich_client::model::SharedLink>, reqwest::Error> {
        let Some(res) = self.admin_get("/shared-links").await else {
            return Ok(None);
        };

        // error_for_status() converts non-2xx into Err(reqwest::Error),
        // so callers can distinguish "link not found" from "API forbidden/unavailable"
        let res = res.error_for_status().map_err(|e| {
            static WARN_ONCE: std::sync::Once = std::sync::Once::new();
            WARN_ONCE.call_once(|| {
                eprintln!("warning: Admin API /shared-links failed: {} — slug and password detection will use fallback heuristics", e);
            });
            e
        })?;

        let links: Vec<crate::immich_client::model::SharedLink> = match res.json().await {
            Ok(l) => l,
            Err(_) => return Ok(None),
        };

        Ok(links
            .into_iter()
            .find(|link| link.key == key_or_slug || link.slug.as_deref() == Some(key_or_slug)))
    }

    /// Fetches the `/shared-links/me` endpoint.
    ///
    /// Tries the provided identifier as a `key` first. On 401, falls back
    /// to querying the admin API to check whether the identifier is a slug,
    /// and retries with the slug parameter if so.
    /// Fetches `/shared-links/me`, optionally forwarding an `immich_shared_link_token`
    /// cookie obtained from a prior `POST /shared-links/login`.
    pub async fn fetch_share_me(
        &self,
        key_or_slug: &str,
        share_token: Option<&str>,
    ) -> Result<(reqwest::StatusCode, String), reqwest::Error> {
        let params = vec![("key", key_or_slug)];

        let url = self.build_url("/shared-links/me", &params);
        let mut req = self.http_client.get(&url);
        if let Some(token) = share_token {
            req = req.header("cookie", format!("immich_shared_link_token={}", token));
        }
        let res = req.send().await?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();

        // On 401, check whether the identifier is actually a slug
        if status == 401 {
            let is_slug = match self.get_admin_shared_link(key_or_slug).await {
                Ok(Some(link)) => link.slug.as_deref() == Some(key_or_slug),
                // No admin key or API error — fall back to text-based detection
                _ => text.contains("Invalid share key"),
            };

            if is_slug {
                let slug_url = self.build_url("/shared-links/me", &[("slug", key_or_slug)]);
                let mut slug_req = self.http_client.get(&slug_url);
                if let Some(token) = share_token {
                    slug_req =
                        slug_req.header("cookie", format!("immich_shared_link_token={}", token));
                }
                if let Ok(r) = slug_req.send().await {
                    return Ok((r.status(), r.text().await.unwrap_or_default()));
                }
            }
        }

        Ok((status, text))
    }

    /// Fetches all assets for a shared album using the timeline API.
    /// In Immich v3, SharedLinkResponseDto no longer includes album assets inline;
    /// they must be fetched via GET /timeline/buckets + GET /timeline/bucket.
    pub async fn fetch_album_assets(
        &self,
        album_id: &str,
        key: &str,
        share_token: Option<&str>,
    ) -> Result<Vec<crate::immich_client::model::Asset>, String> {
        use crate::immich_client::model::{TimeBucket, TimeBucketData};
        use futures_util::StreamExt;

        // Step 1: Get all time buckets for this album
        let buckets_url =
            self.build_url("/timeline/buckets", &[("albumId", album_id), ("key", key)]);
        let mut req = self.http_client.get(&buckets_url);
        if let Some(token) = share_token {
            req = req.header("cookie", format!("immich_shared_link_token={}", token));
        }
        let res = req.send().await.map_err(|e| e.to_string())?;
        let status = res.status();
        if !status.is_success() {
            let text = res.text().await.unwrap_or_default();
            return Err(format!("timeline/buckets failed: {} {}", status, text));
        }
        let buckets: Vec<TimeBucket> = res.json().await.map_err(|e| e.to_string())?;

        // Step 2: Fetch each bucket concurrently (columnar format)
        let share_token_owned = share_token.map(String::from);

        // Pre-build all bucket URLs to avoid lifetime issues with async closures
        let bucket_tasks: Vec<(String, String, Client)> = buckets
            .iter()
            .map(|bucket| {
                let bucket_url = self.build_url(
                    "/timeline/bucket",
                    &[
                        ("albumId", album_id),
                        ("key", key),
                        ("timeBucket", &bucket.time_bucket),
                    ],
                );
                (
                    bucket.time_bucket.clone(),
                    bucket_url,
                    self.http_client.clone(),
                )
            })
            .collect();

        let mut fetches = futures_util::stream::iter(bucket_tasks.into_iter().map(
            |(time_bucket, bucket_url, client)| {
                let token = share_token_owned.clone();

                async move {
                    let mut req = client.get(&bucket_url);
                    if let Some(ref t) = token {
                        req = req.header("cookie", format!("immich_shared_link_token={}", t));
                    }
                    match req.send().await {
                        Ok(res) if res.status().is_success() => res
                            .json::<TimeBucketData>()
                            .await
                            .ok()
                            .map(|d| (time_bucket, d)),
                        Ok(res) => {
                            eprintln!(
                                "Warning: timeline/bucket failed for {}: {}",
                                time_bucket,
                                res.status()
                            );
                            None
                        }
                        Err(e) => {
                            eprintln!(
                                "Warning: timeline/bucket request error for {}: {}",
                                time_bucket, e
                            );
                            None
                        }
                    }
                }
            },
        ))
        .buffer_unordered(8);

        let mut bucket_results = Vec::new();
        while let Some(result) = fetches.next().await {
            if let Some(pair) = result {
                bucket_results.push(pair);
            }
        }

        // Sort buckets chronologically (latest first) to preserve timeline order
        bucket_results.sort_by(|a, b| b.0.cmp(&a.0));

        // Step 3: Convert columnar data to Asset structs
        let mut all_assets = Vec::new();
        for (_time_bucket, data) in bucket_results {
            for i in 0..data.id.len() {
                let is_image = data.is_image.get(i).copied().unwrap_or(true);
                let asset = crate::immich_client::model::Asset {
                    id: data.id[i].clone(),
                    r#type: if is_image { "IMAGE" } else { "VIDEO" }.to_string(),
                    original_file_name: None,
                    original_mime_type: None,
                    file_created_at: data.file_created_at.get(i).cloned(),
                    owner_id: data.owner_id.get(i).cloned(),
                    is_trashed: Some(false),
                    width: None,
                    height: None,
                    ratio: data.ratio.get(i).copied(),
                    exif_info: None,
                    db_id: None,
                    owner: None,
                    tags: None,
                };
                all_assets.push(asset);
            }
        }
        Ok(all_assets)
    }

    /// Authenticates a password-protected shared link via `POST /shared-links/login`.
    /// Returns `Ok(Some(token))` on success (the `immich_shared_link_token` from the
    /// Set-Cookie header), `Ok(None)` on auth failure (wrong password / invalid link),
    /// and `Err` on transport errors.
    pub async fn login_share_link(
        &self,
        key_or_slug: &str,
        password: &str,
    ) -> Result<Option<(String, String)>, reqwest::Error> {
        let params = vec![("key", key_or_slug)];
        let url = self.build_url("/shared-links/login", &params);
        let body = serde_json::json!({ "password": password });
        let res = self.http_client.post(&url).json(&body).send().await?;

        if !res.status().is_success() {
            // Try as slug
            let slug_url = self.build_url("/shared-links/login", &[("slug", key_or_slug)]);
            let slug_res = self.http_client.post(&slug_url).json(&body).send().await?;
            if !slug_res.status().is_success() {
                return Ok(None);
            }
            // Extract immich_shared_link_token from Set-Cookie
            let token = Self::extract_share_token(&slug_res);
            let text = slug_res.text().await.unwrap_or_default();
            return Ok(token.map(|t| (t, text)));
        }

        let token = Self::extract_share_token(&res);
        let text = res.text().await.unwrap_or_default();
        Ok(token.map(|t| (t, text)))
    }

    /// Extract `immich_shared_link_token` value from Set-Cookie headers.
    fn extract_share_token(res: &reqwest::Response) -> Option<String> {
        for val in res.headers().get_all("set-cookie") {
            if let Ok(s) = val.to_str() {
                if let Some(rest) = s.strip_prefix("immich_shared_link_token=") {
                    if let Some(token) = rest.split(';').next() {
                        if !token.is_empty() {
                            return Some(token.to_string());
                        }
                    }
                }
            }
        }
        None
    }
}

/// Reads the proxy's own `immich_share_token_{b64_key}` cookie, which stores the
/// `immich_shared_link_token` value obtained from `POST /shared-links/login` for
/// password-protected shares.
pub fn get_cookie_share_token(headers: &axum::http::HeaderMap, key: &str) -> Option<String> {
    use base64::Engine;
    let b64_key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key);
    let prefix = format!("immich_share_token_{}=", b64_key);

    headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookie_str| {
            cookie_str
                .split(';')
                .map(|s| s.trim())
                .find(|s| s.starts_with(&prefix))
                .and_then(|s| {
                    let encoded = &s[prefix.len()..];
                    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
                        .decode(encoded)
                        .ok()?;
                    String::from_utf8(decoded).ok()
                })
        })
}
