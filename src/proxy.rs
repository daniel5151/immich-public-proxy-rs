use crate::immich_client::client::{ImmichClient, get_cookie_password};
use axum::{
    body::Body,
    extract::{Form, Path, Query},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
};
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
    }
}

#[derive(Deserialize)]
pub struct ProxyQuery {
    /// The "slug key" (sk). If the user accessed the album via a custom slug (e.g. `withpass`),
    /// their password is saved in a cookie named `immich_pwd_withpass`.
    /// The UI natively resolves that slug into Immich's real, internal encryption key,
    /// so that proxy paths use the direct `key` (e.g. `/share/photo/{real_key}/...`).
    /// The frontend passes `?sk=withpass` backwards as a hint, indicating to the proxy
    /// which cookie to check to authenticate the underlying stream.
    sk: Option<String>,
}

#[derive(Deserialize)]
pub struct UnlockPayload {
    key: String,
    password: String,
}

pub async fn unlock_share_handler(Form(payload): Form<UnlockPayload>) -> impl IntoResponse {
    let client = ImmichClient::new();
    let mut params = vec![
        ("key", payload.key.as_str()),
        ("password", payload.password.as_str()),
    ];
    let mut url = client.build_url("/shared-links/me", &params);
    let mut success = false;

    if let Ok(r) = client.http_client.get(&url).send().await {
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        if status == 401 && text.contains("Invalid share key") {
            params[0] = ("slug", payload.key.as_str());
            url = client.build_url("/shared-links/me", &params);
            if let Ok(r2) = client.http_client.get(&url).send().await {
                success = r2.status().is_success();
            }
        } else {
            success = status.is_success();
        }
    }

    if success {
        let cookie = format!(
            "immich_pwd_{}={}; Path=/; HttpOnly",
            payload.key, payload.password
        );
        let mut resp = Redirect::to(&format!("/share/{}", payload.key)).into_response();
        resp.headers_mut()
            .insert(axum::http::header::SET_COOKIE, cookie.parse().unwrap());
        return resp;
    }
    Redirect::to(&format!("/share/{}", payload.key)).into_response()
}

pub async fn proxy_photo(
    headers: HeaderMap,
    Path((key, id, size)): Path<(String, String, String)>,
    Query(query): Query<ProxyQuery>,
) -> impl IntoResponse {
    proxy_photo_impl(headers, key, id, size, query.sk).await
}

pub async fn proxy_photo_no_size(
    headers: HeaderMap,
    Path((key, id)): Path<(String, String)>,
    Query(query): Query<ProxyQuery>,
) -> impl IntoResponse {
    proxy_photo_impl(headers, key, id, "preview".to_string(), query.sk).await
}

async fn proxy_photo_impl(
    headers: HeaderMap,
    key: String,
    id: String,
    size_str: String,
    sk: Option<String>,
) -> impl IntoResponse {
    let client = ImmichClient::new();

    // Use the `sk` hint from the frontend to look up the password cookie for custom slugs.
    // If no `sk` was needed or provided, default to looking it up using the real `key` itself.
    let cookie_password = get_cookie_password(&headers, sk.as_deref().unwrap_or(&key));

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
    Query(query): Query<ProxyQuery>,
) -> impl IntoResponse {
    let client = ImmichClient::new();

    // Use the `sk` hint from the frontend to look up the password cookie for custom slugs.
    // If no `sk` was needed or provided, default to looking it up using the real `key` itself.
    let cookie_password = get_cookie_password(&headers, query.sk.as_deref().unwrap_or(&key));

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
    Query(proxy_query): Query<ProxyQuery>,
) -> impl IntoResponse {
    let client = ImmichClient::new();

    // Use the `sk` hint from the frontend to look up the password cookie for custom slugs.
    // If no `sk` was needed or provided, default to looking it up using the real `key` itself.
    let cookie_password = get_cookie_password(&headers, proxy_query.sk.as_deref().unwrap_or(&key));

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
        ids_str.split(',').map(|s| s.to_string()).collect()
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
