#![recursion_limit = "512"]

mod api;
mod dto;
mod immich_client;
mod proxy;

use crate::proxy::ProxyRoutes as _;
use axum::{
    Router,
    extract::Path,
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::get,
};

async fn serve_share_html(Path(key): Path<String>, headers: HeaderMap) -> Response {
    let details_res = api::get_share_details::get_share_details(key.clone(), None, &headers).await;

    let site_root = std::env::var("LEPTOS_SITE_ROOT").unwrap_or_else(|_| "target/site".to_string());
    let index_path = std::path::Path::new(&site_root).join("index.html");

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

            let public_base_url = details.public_base_url.trim_end_matches('/').to_string();
            let current_url = format!("{}/share/{}", public_base_url, details.request_key);

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
                        public_base_url, details.link.key, id
                    )
                })
                .unwrap_or_default();

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

    Html(html_content).into_response()
}

async fn serve_index() -> Response {
    let site_root = std::env::var("LEPTOS_SITE_ROOT").unwrap_or_else(|_| "target/site".to_string());
    let index_path = std::path::Path::new(&site_root).join("index.html");

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
    let addr_str =
        std::env::var("LEPTOS_SITE_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".to_string());
    let addr: std::net::SocketAddr = addr_str.parse().expect("Invalid bind address");

    let site_root = std::env::var("LEPTOS_SITE_ROOT").unwrap_or_else(|_| "target/site".to_string());

    let app = Router::new()
        // API routes
        .route(
            "/api/share/{key}",
            get(api::get_share_details::get_share_details_handler),
        )
        // Proxy routes
        .proxy_routes()
        // Meta SEO routes for shares
        .route("/share/{key}", get(serve_share_html))
        .route("/s/{key}", get(serve_share_html))
        // Static files serving
        .fallback_service(
            tower_http::services::ServeDir::new(&site_root).fallback(get(serve_index)),
        )
        .layer(axum::extract::DefaultBodyLimit::disable())
        .layer(axum::middleware::map_response(
            |mut response: Response| async move {
                static CSP_HEADER: std::sync::OnceLock<axum::http::HeaderValue> =
                    std::sync::OnceLock::new();
                let csp_value = CSP_HEADER.get_or_init(|| {
                    let connect_src = if cfg!(debug_assertions) {
                        "'self' ws://127.0.0.1:5173" // Vite dev ws
                    } else {
                        "'self'"
                    };

                    let csp = format!(
                        "default-src 'none'; \
                         script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; \
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
                    axum::http::HeaderValue::from_str(&csp).unwrap()
                });
                response.headers_mut().insert(
                    axum::http::header::CONTENT_SECURITY_POLICY,
                    csp_value.clone(),
                );
                response
            },
        ));

    println!("Listening on http://{}", &addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}
