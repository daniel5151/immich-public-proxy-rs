use reqwest::{Client, Url};

pub struct ImmichClient {
    pub api_url: String,
    pub http_client: Client,
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

        Self {
            api_url,
            http_client,
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
}

pub fn get_cookie_password(headers: &axum::http::HeaderMap, key: &str) -> Option<String> {
    headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookie_str| {
            let prefix = format!("immich_pwd_{}=", key);
            cookie_str
                .split(';')
                .map(|s| s.trim())
                .find(|s| s.starts_with(&prefix))
                .map(|s| s[prefix.len()..].to_string())
        })
}
