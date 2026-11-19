#![recursion_limit = "512"]

mod api;
mod dto;
mod immich_client;
mod proxy;
mod security;

use crate::api::get_share_details::ShareDetailsError;
use crate::proxy::ProxyRoutes as _;
use axum::{
    Router,
    extract::{ConnectInfo, DefaultBodyLimit, Path},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use std::net::SocketAddr;

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

async fn serve_share_html(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(key): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !security::is_safe_param(&key) {
        return (StatusCode::BAD_REQUEST, "Invalid share key").into_response();
    }

    let client_ip = security::extract_client_ip(&headers, peer.ip());
    if let Err(retry_after) = security::probe_limiter().check_blocked(client_ip) {
        return security::rate_limited_response(retry_after, "Too many requests");
    }

    let details_res = api::get_share_details::get_share_details(key.clone(), &headers).await;
    if matches!(
        details_res,
        Err(ShareDetailsError::InvalidKey | ShareDetailsError::NotAccessible)
    ) {
        security::probe_limiter().penalize(client_ip);
    }

    static SITE_ROOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let site_root = SITE_ROOT.get_or_init(|| {
        std::env::var("LEPTOS_SITE_ROOT").unwrap_or_else(|_| "target/site".to_string())
    });
    let index_path = std::path::Path::new(site_root).join("index.html");

    let mut html_content = match tokio::fs::read_to_string(&index_path).await {
        Ok(content) => content,
        Err(e) => {
            eprintln!("Failed to read index.html from {:?}: {}", index_path, e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read index.html",
            )
                .into_response();
        }
    };

    if let Ok(details) = details_res {
        if !details.password_required {
            let title = details
                .link
                .description
                .clone()
                .or_else(|| {
                    details
                        .link
                        .album
                        .as_ref()
                        .and_then(|a| a.album_name.clone())
                })
                .unwrap_or_else(|| "Shared Files".to_string());

            let description = details
                .link
                .album
                .as_ref()
                .and_then(|a| a.description.clone())
                .unwrap_or_else(|| {
                    format!(
                        "Shared album containing {} item(s)",
                        details.link.assets.len()
                    )
                });

            let ipp_public_base_url = details
                .ipp_public_base_url
                .trim_end_matches('/')
                .to_string();
            let current_url = format!("{}/share/{}", ipp_public_base_url, details.request_key);

            let cover_asset_id = details
                .link
                .album
                .as_ref()
                .and_then(|a| a.album_thumbnail_asset_id.clone())
                .or_else(|| details.link.assets.first().map(|a| a.id.clone()));

            let cover_image_url = cover_asset_id
                .map(|id| {
                    format!(
                        "{}/share/photo/{}/{}/preview",
                        ipp_public_base_url, details.link.key, id
                    )
                })
                .unwrap_or_default();

            let title = html_escape(&title);
            let description = html_escape(&description);
            let cover_image_url = html_escape(&cover_image_url);
            let current_url = html_escape(&current_url);

            let meta_tags = format!(
                r#"<meta name="description" content="{}" />
<meta property="og:title" content="{}" />
<meta property="og:description" content="{}" />
<meta property="og:image" content="{}" />
<meta property="og:url" content="{}" />
<meta property="og:type" content="website" />
<meta name="twitter:card" content="summary_large_image" />
<meta name="twitter:title" content="{}" />
<meta name="twitter:description" content="{}" />
<meta name="twitter:image" content="{}" />"#,
                description,
                title,
                description,
                cover_image_url,
                current_url,
                title,
                description,
                cover_image_url
            );

            if let (Some(start), Some(end)) =
                (html_content.find("<title>"), html_content.find("</title>"))
            {
                if start < end {
                    html_content
                        .replace_range(start..end + 8, &format!("<title>{}</title>", title));
                }
            }

            if let Some(pos) = html_content.find("</head>") {
                html_content.insert_str(pos, &meta_tags);
            }
        }
    }

    let mut resp = Html(html_content).into_response();
    security::insert_private_no_store(resp.headers_mut());
    resp
}

async fn serve_index() -> Response {
    static SITE_ROOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let site_root = SITE_ROOT.get_or_init(|| {
        std::env::var("LEPTOS_SITE_ROOT").unwrap_or_else(|_| "target/site".to_string())
    });
    let index_path = std::path::Path::new(site_root).join("index.html");

    match tokio::fs::read_to_string(&index_path).await {
        Ok(content) => Html(content).into_response(),
        Err(e) => {
            eprintln!("Failed to read index.html from {:?}: {}", index_path, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to read index.html",
            )
                .into_response()
        }
    }
}

#[tokio::main]
async fn main() {
    // Validate required upstream config eagerly on startup.
    immich_client::client::ImmichClient::init_and_validate();

    let addr_str =
        std::env::var("LEPTOS_SITE_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());
    let addr: SocketAddr = addr_str.parse().expect("Invalid bind address");

    let site_root = std::env::var("LEPTOS_SITE_ROOT").unwrap_or_else(|_| "target/site".to_string());

    let app = Router::new()
        // API routes
        .route(
            "/api/share/{key}",
            get(api::get_share_details::get_share_details_handler),
        )
        // Proxy routes (upload route sets its own IPP_MAX_UPLOAD_BYTES body limit)
        .proxy_routes()
        // Meta SEO routes for shares
        .route("/share/{key}", get(serve_share_html))
        .route("/s/{key}", get(serve_share_html))
        // Static files serving
        .fallback_service(
            tower_http::services::ServeDir::new(&site_root).fallback(get(serve_index)),
        )
        // Strict default body cap for all non-upload routes (prevents OOM on /share/unlock).
        .layer(DefaultBodyLimit::max(64 * 1024))
        .layer(axum::middleware::map_response(
            |mut response: Response| async move {
                static CSP_HEADER: std::sync::OnceLock<HeaderValue> = std::sync::OnceLock::new();
                let csp_value = CSP_HEADER.get_or_init(|| {
                    let connect_src = if cfg!(debug_assertions) {
                        "'self' ws://127.0.0.1:5173" // Vite dev ws
                    } else {
                        "'self'"
                    };

                    let csp = format!(
                        "default-src 'none'; \
                         script-src 'self'; \
                         worker-src 'self' blob:; \
                         style-src 'self' 'unsafe-inline'; \
                         img-src 'self' data:; \
                         media-src 'self'; \
                         font-src 'self' data:; \
                         connect-src {connect_src}; \
                         frame-ancestors 'none'; \
                         base-uri 'self'; \
                         form-action 'self'"
                    );
                    HeaderValue::from_str(&csp).unwrap()
                });

                let headers = response.headers_mut();
                // Respect stricter per-route CSP (e.g., `default-src 'none'; sandbox` on proxied media).
                if !headers.contains_key(header::CONTENT_SECURITY_POLICY) {
                    headers.insert(header::CONTENT_SECURITY_POLICY, csp_value.clone());
                }
                headers.insert(
                    header::X_CONTENT_TYPE_OPTIONS,
                    HeaderValue::from_static("nosniff"),
                );
                headers.insert(
                    header::REFERRER_POLICY,
                    HeaderValue::from_static("no-referrer"),
                );
                headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
                headers.insert(
                    HeaderName::from_static("permissions-policy"),
                    HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
                );
                headers.insert(
                    HeaderName::from_static("cross-origin-opener-policy"),
                    HeaderValue::from_static("same-origin"),
                );
                headers.insert(
                    HeaderName::from_static("cross-origin-resource-policy"),
                    HeaderValue::from_static("same-origin"),
                );
                response
            },
        ));

    println!("Listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap();
}
