#[cfg(feature = "ssr")]
pub mod ssr {
    use crate::immich::ssr::ImmichClient;
    use axum::{
        body::Body,
        extract::{Form, Path},
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Redirect},
    };
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub struct UnlockPayload {
        key: String,
        password: String,
    }

    pub async fn unlock_share_handler(Form(payload): Form<UnlockPayload>) -> impl IntoResponse {
        let client = ImmichClient::new();
        let params = vec![
            ("key", payload.key.as_str()),
            ("password", payload.password.as_str()),
        ];
        let url = client.build_url("/shared-links/me", &params);
        if let Ok(res) = client.http_client.get(&url).send().await {
            if res.status().is_success() {
                let cookie = format!(
                    "immich_pwd_{}={}; Path=/; HttpOnly",
                    payload.key, payload.password
                );
                let mut resp = Redirect::to(&format!("/share/{}", payload.key)).into_response();
                resp.headers_mut()
                    .insert(axum::http::header::SET_COOKIE, cookie.parse().unwrap());
                return resp;
            }
        }
        Redirect::to(&format!("/share/{}", payload.key)).into_response()
    }

    pub async fn proxy_photo(
        headers: HeaderMap,
        Path((key, id, size)): Path<(String, String, Option<String>)>,
    ) -> impl IntoResponse {
        let client = ImmichClient::new();
        let size_str = size.unwrap_or_else(|| "preview".to_string());

        let cookie_str = headers
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let prefix = format!("immich_pwd_{}=", key);
        let cookie_password = cookie_str
            .split(';')
            .map(|s| s.trim())
            .find(|s| s.starts_with(&prefix))
            .map(|s| s[prefix.len()..].to_string());

        let mut params = vec![("key", key.as_str()), ("size", size_str.as_str())];
        if let Some(ref pwd) = cookie_password {
            params.push(("password", pwd.as_str()));
        }

        let _url = client.build_url(&format!("/assets/{}/thumbnail", id), &params);

        let res = match client.http_client.get(&_url).send().await {
            Ok(res) => res,
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        };

        let mut builder = axum::response::Response::builder().status(res.status());
        for (k, v) in res.headers() {
            if ["content-type", "content-length", "etag", "last-modified"].contains(&k.as_str()) {
                builder = builder.header(k.clone(), v.clone());
            }
        }
        builder.body(Body::from_stream(res.bytes_stream())).unwrap()
    }

    pub async fn proxy_video(
        headers: HeaderMap,
        Path((key, id)): Path<(String, String)>,
    ) -> impl IntoResponse {
        let client = ImmichClient::new();

        let cookie_str = headers
            .get(axum::http::header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let prefix = format!("immich_pwd_{}=", key);
        let cookie_password = cookie_str
            .split(';')
            .map(|s| s.trim())
            .find(|s| s.starts_with(&prefix))
            .map(|s| s[prefix.len()..].to_string());

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
}
