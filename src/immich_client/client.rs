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

    /// Queries the admin /shared-links endpoint to find a link by its key or slug.
    pub async fn get_admin_shared_link(
        &self,
        key_or_slug: &str,
    ) -> Result<Option<crate::immich_client::model::SharedLink>, reqwest::Error> {
        let Some(admin_key) = &self.admin_api_key else {
            return Ok(None);
        };

        let url = self.build_url("/shared-links", &[]);
        let res = self
            .http_client
            .get(&url)
            .header("x-api-key", admin_key)
            .send()
            .await?;

        if !res.status().is_success() {
            return Ok(None);
        }

        if let Ok(links) = res
            .json::<Vec<crate::immich_client::model::SharedLink>>()
            .await
        {
            for link in links {
                if link.key == key_or_slug || link.slug.as_deref() == Some(key_or_slug) {
                    return Ok(Some(link));
                }
            }
        }
        Ok(None)
    }

    /// Fetches the `/shared-links/me` endpoint.
    /// It first tries using the provided identifier as a `key`.
    /// If the server responds with 401 "Invalid share key", it automatically retries with `slug`.
    pub async fn fetch_share_me(
        &self,
        key_or_slug: &str,
        password: Option<&str>,
    ) -> Result<(reqwest::StatusCode, String), reqwest::Error> {
        let mut params = vec![("key", key_or_slug)];
        if let Some(p) = password {
            params.push(("password", p));
        }

        let mut url = self.build_url("/shared-links/me", &params);
        let res = self.http_client.get(&url).send().await?;

        let mut status = res.status();
        let mut text = res.text().await.unwrap_or_default();

        if status == 401 {
            // As a fallback for slugs, we query our admin sidecar.
            if let Ok(Some(link)) = self.get_admin_shared_link(key_or_slug).await {
                if link.slug.as_deref() == Some(key_or_slug) {
                    params[0] = ("slug", key_or_slug);
                    url = self.build_url("/shared-links/me", &params);
                    if let Ok(r) = self.http_client.get(&url).send().await {
                        status = r.status();
                        text = r.text().await.unwrap_or_default();
                    }
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
