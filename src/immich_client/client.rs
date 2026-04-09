use reqwest::Client;
use reqwest::Url;

pub struct ImmichClient {
    pub api_url: String,
    pub http_client: Client,
    pub admin_api_key: Option<String>,
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
        let http_client = CLIENT.get_or_init(Client::new).clone();

        static ADMIN_API_KEY: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        let admin_api_key = ADMIN_API_KEY
            .get_or_init(|| std::env::var("IMMICH_API_KEY").ok())
            .clone();

        Self {
            api_url,
            http_client,
            admin_api_key,
        }
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

    /// Sends an authenticated GET request using the admin API key.
    /// Returns `None` if there is no admin API key configured.
    pub async fn admin_get(&self, path: &str) -> Option<reqwest::Response> {
        let admin_key = self.admin_api_key.as_ref()?;
        let url = self.build_url(path, &[]);
        self.http_client
            .get(&url)
            .header("x-api-key", admin_key)
            .send()
            .await
            .ok()
    }

    /// Sends an authenticated POST request using the admin API key.
    /// Returns `None` if there is no admin API key configured.
    pub async fn admin_post(
        &self,
        path: &str,
        body: &impl serde::Serialize,
    ) -> Option<reqwest::Response> {
        let admin_key = self.admin_api_key.as_ref()?;
        let url = self.build_url(path, &[]);
        self.http_client
            .post(&url)
            .header("x-api-key", admin_key)
            .json(body)
            .send()
            .await
            .ok()
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
    pub async fn fetch_share_me(
        &self,
        key_or_slug: &str,
        password: Option<&str>,
    ) -> Result<(reqwest::StatusCode, String), reqwest::Error> {
        let mut params = vec![("key", key_or_slug)];
        if let Some(p) = password {
            params.push(("password", p));
        }

        let url = self.build_url("/shared-links/me", &params);
        let res = self.http_client.get(&url).send().await?;
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
                params[0] = ("slug", key_or_slug);
                let slug_url = self.build_url("/shared-links/me", &params);
                if let Ok(r) = self.http_client.get(&slug_url).send().await {
                    return Ok((r.status(), r.text().await.unwrap_or_default()));
                }
            }
        }

        Ok((status, text))
    }
}

pub fn get_cookie_password(headers: &axum::http::HeaderMap, key: &str) -> Option<String> {
    use base64::Engine;
    let b64_key = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key);
    let prefix = format!("immich_pwd_{}=", b64_key);

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
