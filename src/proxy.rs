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
    let encoded_filename = urlencoding::encode(&filename);
    headers.insert(
        axum::http::header::CONTENT_DISPOSITION,
        format!("attachment; filename*=UTF-8''{}", encoded_filename)
            .parse()
            .unwrap(),
    );

    (headers, Body::from_stream(res.bytes_stream())).into_response()
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
    let cookie_password = get_cookie_password(&headers, &key);

    let mut params = vec![("key", key.as_str())];
    if let Some(ref pwd) = cookie_password {
        params.push(("password", pwd.as_str()));
    }

    // Extract crucial headers from the incoming request.
    // Content-Type is mandatory because it contains the `boundary=---...` string.
    let content_type = headers.get(axum::http::header::CONTENT_TYPE).cloned();
    let content_length = headers.get(axum::http::header::CONTENT_LENGTH).cloned();

    // Stream the raw axum body directly into the reqwest body.
    // This streams the payload chunk-by-chunk without loading the whole file into RAM.
    let stream = req.into_body().into_data_stream();
    let reqwest_body = reqwest::Body::wrap_stream(stream);

    // Forward the streamed request to Immich
    let url = client.build_url("/assets", &params);
    let mut out_req = client.http_client.post(&url).body(reqwest_body);

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

    // For album shares, add the uploaded asset to the album
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

    if let Some(album_id) = share_link
        .as_ref()
        .and_then(|link| link.album.as_ref())
        .map(|album| album.id.as_str())
    {
        let album_url = client.build_url(&format!("/albums/{}/assets", album_id), &params);
        let album_res = client
            .http_client
            .put(&album_url)
            .json(&serde_json::json!({ "ids": [asset_id] }))
            .send()
            .await;

        if let Err(e) = &album_res {
            eprintln!(
                "failed to add asset {} to album {}: {}",
                asset_id, album_id, e
            );
        }
    }

    StatusCode::OK.into_response()
}
