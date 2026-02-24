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
        Path((key, id, size)): Path<(String, String, String)>,
    ) -> impl IntoResponse {
        proxy_photo_impl(headers, key, id, Some(size)).await
    }

    pub async fn proxy_photo_no_size(
        headers: HeaderMap,
        Path((key, id)): Path<(String, String)>,
    ) -> impl IntoResponse {
        proxy_photo_impl(headers, key, id, None).await
    }

    async fn proxy_photo_impl(
        headers: HeaderMap,
        key: String,
        id: String,
        size: Option<String>,
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

        let _url = client.build_url(&subpath, &params);

        let res = match client.http_client.get(&_url).send().await {
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

    pub async fn download_all(headers: HeaderMap, Path(key): Path<String>) -> impl IntoResponse {
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

        let url = client.build_url("/shared-links/me", &params);
        let res = client.http_client.get(&url).send().await;
        let mut share: crate::immich::SharedLink = match res {
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
                    if let Ok(album_data) = album_res.json::<crate::immich::Album>().await {
                        share.assets = album_data.assets;
                    }
                }
            }
        }

        let title = share
            .description
            .clone()
            .or_else(|| share.album.as_ref().and_then(|a| a.album_name.clone()))
            .unwrap_or_else(|| "photos".to_string());
        let sanitized_title = title.replace(|c: char| !c.is_alphanumeric() && c != '-', "_");
        let filename = format!("{}.zip", sanitized_title);

        let (w, r) = tokio::io::duplex(64 * 1024);
        let body = Body::from_stream(tokio_util::io::ReaderStream::new(r));

        let params_owned: Vec<(String, String)> = params
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        tokio::spawn(async move {
            use leptos;
            use tokio_util::compat::TokioAsyncWriteCompatExt;
            let mut zip = async_zip::tokio::write::ZipFileWriter::new(w.compat_write());

            for asset in share.assets {
                let mut p = params_owned.clone();
                p.push(("size".to_string(), "original".to_string()));
                let p_refs: Vec<(&str, &str)> =
                    p.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

                let original_name = asset.original_file_name.unwrap_or_else(|| asset.id.clone());
                let ext = if !original_name.contains('.') {
                    if asset.r#type == "VIDEO" {
                        ".mp4"
                    } else {
                        ".jpg"
                    }
                } else {
                    ""
                };
                let file_name = format!("{}{}", original_name, ext);

                let builder = async_zip::ZipEntryBuilder::new(
                    file_name.into(),
                    async_zip::Compression::Deflate,
                );

                let asset_url =
                    client.build_url(&format!("/assets/{}/original", asset.id), &p_refs);
                if let Ok(mut asset_res) = client.http_client.get(&asset_url).send().await {
                    if asset_res.status().is_success() {
                        if let Ok(mut entry_writer) = zip.write_entry_stream(builder).await {
                            while let Ok(Some(chunk)) = asset_res.chunk().await {
                                use futures_util::io::AsyncWriteExt;
                                if entry_writer.write_all(&chunk).await.is_err() {
                                    break;
                                }
                            }
                            let _ = entry_writer.close().await;
                        }
                    }
                }
            }
            if let Err(e) = zip.close().await {
                leptos::logging::log!("Zip close error: {:?}", e);
            }
        });

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

        (headers, body).into_response()
    }
}
