mod api;
mod app;
mod dto;
mod immich_client;
#[cfg(feature = "ssr")]
mod proxy;

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use crate::app::*;
    use crate::proxy::ProxyRoutes as _;
    use axum::Router;
    use leptos::logging::log;
    use leptos::prelude::*;
    use leptos_axum::{LeptosRoutes, generate_route_list};

    fn shell(options: LeptosOptions) -> impl IntoView {
        use leptos_meta::MetaTags;
        view! {
            <!DOCTYPE html>
            <html lang="en">
                <head>
                    <meta charset="utf-8"/>
                    <meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1.0"/>
                    <AutoReload options=options.clone() />
                    <HydrationScripts options/>
                    <MetaTags/>
                </head>
                <body>
                    <App/>
                </body>
            </html>
        }
    }

    let conf = get_configuration(None).unwrap();
    let addr = conf.leptos_options.site_addr;
    let leptos_options = conf.leptos_options;
    // Generate the list of routes in your Leptos App
    let routes = generate_route_list(App);

    let app = Router::new()
        .proxy_routes()
        .layer(axum::extract::DefaultBodyLimit::disable())
        .leptos_routes(&leptos_options, routes, {
            let leptos_options = leptos_options.clone();
            move || shell(leptos_options.clone())
        })
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(leptos_options)
        .layer(axum::middleware::map_response(
            |mut response: axum::response::Response| async move {
                // Content-Security-Policy:
                // - script-src: 'unsafe-inline' is required by Leptos HydrationScripts.
                //   Our own code has no inline scripts (moved to web.js), so this can
                //   be replaced with nonce-based CSP once Leptos nonce support is configured.
                //   'wasm-unsafe-eval' is required for Leptos WASM hydration.
                // - style-src 'unsafe-inline': Leptos hydration injects inline style attrs
                // - img-src data:: LightGallery uses data: URIs for some icons
                // - frame-ancestors 'none': prevents clickjacking via iframing
                //
                // In debug builds, Leptos AutoReload creates a blob: Web Worker and
                // connects to ws://127.0.0.1:<reload-port>, so the dev CSP relaxes
                // worker-src and connect-src accordingly.
                let connect_src = if cfg!(debug_assertions) {
                    "'self' ws://127.0.0.1:3001"
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
                     font-src 'self'; \
                     connect-src {connect_src}; \
                     frame-ancestors 'none'; \
                     base-uri 'self'; \
                     form-action 'self'"
                );
                response.headers_mut().insert(
                    axum::http::header::CONTENT_SECURITY_POLICY,
                    axum::http::HeaderValue::from_str(&csp).unwrap(),
                );
                response
            },
        ));

    // run our app with hyper
    // `axum::Server` is a re-export of `hyper::Server`
    log!("listening on http://{}", &addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}

#[cfg(not(feature = "ssr"))]
pub fn main() {} // hydration entry via wasm-bindgen in lib.rs
