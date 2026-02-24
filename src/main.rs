#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::Router;
    use leptos::logging::log;
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use rs::app::*;

    let conf = get_configuration(None).unwrap();
    let addr = conf.leptos_options.site_addr;
    let leptos_options = conf.leptos_options;
    // Generate the list of routes in your Leptos App
    let routes = generate_route_list(App);

    let app = Router::new()
        .route(
            "/share/photo/{key}/{id}/{size}",
            axum::routing::get(rs::proxy::ssr::proxy_photo),
        )
        .route(
            "/share/photo/{key}/{id}",
            axum::routing::get(rs::proxy::ssr::proxy_photo_no_size),
        )
        .route(
            "/share/video/{key}/{id}",
            axum::routing::get(rs::proxy::ssr::proxy_video),
        )
        .route(
            "/share/unlock",
            axum::routing::post(rs::proxy::ssr::unlock_share_handler),
        )
        .route(
            "/share/{key}/download",
            axum::routing::get(rs::proxy::ssr::download_all),
        )
        .leptos_routes(&leptos_options, routes, {
            let leptos_options = leptos_options.clone();
            move || shell(leptos_options.clone())
        })
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(leptos_options);

    // run our app with hyper
    // `axum::Server` is a re-export of `hyper::Server`
    log!("listening on http://{}", &addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}

#[cfg(not(feature = "ssr"))]
pub fn main() {
    // no client-side main function
    // unless we want this to work with e.g., Trunk for pure client-side testing
    // see lib.rs for hydration function instead
}
