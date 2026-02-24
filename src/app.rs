use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::{
    components::{Route, Router, Routes},
    ParamSegment, StaticSegment,
};
use std::collections::HashSet;

pub fn shell(options: LeptosOptions) -> impl IntoView {
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

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <Stylesheet id="leptos" href="/pkg/rs.css"/>
        <Stylesheet href="/style.css"/>
        <Stylesheet href="/lg/lightgallery-bundle.min.css"/>
        <script src="/web.js"/>
        <script src="/lg/lightgallery.min.js"/>
        <script src="/lg/lg-fullscreen.min.js"/>
        <script src="/lg/lg-thumbnail.min.js"/>
        <script src="/lg/lg-video.min.js"/>
        <script src="/lg/lg-zoom.min.js"/>
        <script src="/lg/lg-hash.min.js"/>
        <Title text="Immich Public Proxy"/>

        <Router>
            <main>
                <Routes fallback=|| "Page not found.".into_view()>
                    <Route path=StaticSegment("") view=HomePage/>
                    <Route path=(StaticSegment("share"), ParamSegment("key")) view=SharePage/>
                    <Route path=(StaticSegment("s"), ParamSegment("key")) view=SharePage/>
                </Routes>
            </main>
        </Router>
    }
}

#[component]
fn HomePage() -> impl IntoView {
    view! {
        <div class="container" style="display:flex;justify-content:center;align-items:center;height:100vh;background:#262626;margin:0">
            <a href="https://github.com/alangrainger/immich-public-proxy">
                <img src="/images/ipp.svg" alt="" style="max-width:280px;height:280px;opacity:0.3"/>
            </a>
        </div>
    }
}

#[component]
fn SharePage() -> impl IntoView {
    let params = leptos_router::hooks::use_params_map();
    let key = move || params.with(|p| p.get("key").unwrap_or_default());

    let share_res = Resource::new(key, |k| crate::server_fns::get_share_details(k, None));

    view! {
        <Suspense fallback=move || view! { <div id="loading-spinner"><span class="loader"></span></div> }>
            {move || match share_res.get() {
                Some(Ok(details)) => {
                    if details.password_required {
                        view! {
                            <Password required_key=key() />
                        }.into_any()
                    } else {
                        view! {
                            <Gallery details=details />
                        }.into_any()
                    }
                },
                Some(Err(_)) => view! { <div>"Failed to load."</div> }.into_any(),
                None => view! { <div>"Loading..."</div> }.into_any(),
            }}
        </Suspense>
    }
}

#[component]
fn Password(required_key: String) -> impl IntoView {
    view! {
        <main class="container" style="display:flex;justify-content:center;align-items:center;height:100vh;flex-direction:column;">
            <div style="background:#333;padding:2rem;border-radius:8px;box-shadow:0 4px 6px rgba(0,0,0,0.1);text-align:center;">
                <form id="unlock" method="post" action="/share/unlock" style="display:flex;flex-direction:column;gap:1rem;">
                    <input type="password" name="password" placeholder="Password" aria-label="Password" required autofocus style="padding:0.5rem;font-size:1rem;border:1px solid #555;border-radius:4px;background:#222;color:#fff;" />
                    <input type="hidden" name="key" value=required_key.clone() />
                    <button type="submit" style="padding:0.5rem 1rem;font-size:1rem;background:#007bff;color:#fff;border:none;border-radius:4px;cursor:pointer;">"Unlock"</button>
                </form>
            </div>
        </main>
    }
}

#[component]
fn AssetTile(
    i: usize,
    asset: crate::immich::Asset,
    share_key: String,
    selected_assets: RwSignal<HashSet<String>>,
    on_toggle: ipp_callback::Callback<String>,
) -> impl IntoView {
    let id = asset.id.clone();
    let id_for_selected = id.clone();
    let id_for_toggle = id.clone();

    let thumbnail_url = format!("/share/photo/{}/{}/thumbnail", share_key, id);
    let is_video = asset.r#type == "VIDEO";

    let width = asset.width.unwrap_or(250) as f32;
    let height = asset.height.unwrap_or(250) as f32;
    let aspect_ratio = width / height;
    let flex_basis = format!("{}px", 250.0 * aspect_ratio);

    let is_selected = move || selected_assets.get().contains(&id_for_selected);
    let preview_url = format!("/share/photo/{}/{}/preview", share_key, id);

    view! {
        <div
            class="tile-wrapper"
            class:selected=is_selected
            style=format!("flex-basis: {}; flex-grow: {}", flex_basis, aspect_ratio)
        >
            <div
                class="tile-selector"
                on:click=move |ev| {
                    ev.stop_propagation();
                    ev.prevent_default();
                    on_toggle.run(id_for_toggle.clone());
                }
            ></div>
            <a
                class="gallery-item"
                attr:data-index=i
                href=preview_url
            >
                <img
                    loading="lazy"
                    src=thumbnail_url
                    alt=""
                    onerror="this.closest('a').classList.add('thumb-error')"
                />
                {if is_video {
                    view! { <div class="play-icon"></div> }.into_any()
                } else {
                    view! { <span style="display:none" /> }.into_any()
                }}
            </a>
        </div>
    }
}

mod ipp_callback {
    use leptos::prelude::*;
    pub struct Callback<T: 'static>(StoredValue<Box<dyn Fn(T) + Send + Sync>>);

    impl<T: 'static> Callback<T> {
        pub fn new<F: Fn(T) + Send + Sync + 'static>(f: F) -> Self {
            Self(StoredValue::new(Box::new(f)))
        }
        pub fn run(&self, data: T) {
            self.0.with_value(|f| f(data));
        }
    }

    impl<T: 'static> Clone for Callback<T> {
        fn clone(&self) -> Self {
            *self
        }
    }

    impl<T: 'static> Copy for Callback<T> {}
}

#[component]
fn Gallery(details: crate::server_fns::ShareDetails) -> impl IntoView {
    let link = details.link;
    let share_key = link.key.clone();
    let assets = link.assets.clone();
    let allow_download = link.allow_download.unwrap_or(true);

    let title = link
        .description
        .clone()
        .or_else(|| link.album.as_ref().and_then(|a| a.album_name.clone()))
        .unwrap_or_else(|| "Gallery".to_string());

    let album_description = link.album.as_ref().and_then(|a| a.description.clone());

    let selected_assets = RwSignal::new(HashSet::<String>::new());

    // LightGallery items array format for dynamic mode
    let items_array = assets
        .iter()
        .map(|asset| {
            let key = share_key.clone();
            let preview_url = format!("/share/photo/{}/{}/preview", key, asset.id);
            let thumbnail_url = format!("/share/photo/{}/{}/thumbnail", key, asset.id);
            let download_url = format!("/share/photo/{}/{}/original", key, asset.id);

            if asset.r#type == "VIDEO" {
                serde_json::json!({
                    "video": {
                        "source": [
                            {
                                "src": format!("/share/video/{}/{}", key, asset.id),
                                "type": "video/mp4"
                            }
                        ],
                        "attributes": {
                            "playsinline": true,
                            "controls": true
                        }
                    },
                    "poster": preview_url,
                    "thumb": thumbnail_url,
                    "downloadUrl": download_url
                })
            } else {
                serde_json::json!({
                    "src": preview_url,
                    "thumb": thumbnail_url,
                    "downloadUrl": download_url
                })
            }
        })
        .collect::<Vec<_>>();

    let items_json = serde_json::to_string(&items_array).unwrap();
    let gallery_data = format!(
        "window.GALLERY_DATA = {{ lgConfig: {{ }}, items: {} }};",
        items_json
    );

    let on_toggle_select = ipp_callback::Callback::new(move |id: String| {
        selected_assets.update(|set| {
            if set.contains(&id) {
                set.remove(&id);
            } else {
                set.insert(id);
            }
        });
    });

    let s_key_for_download = share_key.clone();
    let download_selection_url = move || {
        let ids = selected_assets.get();
        if ids.is_empty() {
            "".to_string()
        } else {
            let ids_str = ids.into_iter().collect::<Vec<_>>().join(",");
            format!(
                "/share/{}/download?asset_ids={}",
                s_key_for_download, ids_str
            )
        }
    };

    view! {
        <div id="gallery-root">
            <div id="selection-bar" class:active=move || !selected_assets.get().is_empty()>
                <button class="icon-btn" on:click=move |_| selected_assets.set(HashSet::new())>
                    "✕"
                </button>
                <div class="selection-count">{move || selected_assets.get().len()} " selected"</div>
                <div class="selection-actions">
                    <a class="icon-btn" href=download_selection_url rel="external" title="Download selection">
                        "↓"
                    </a>
                </div>
            </div>

            <div id="header">
                <h1>{title}</h1>
                <div class="header-actions">
                    <div id="download-all" style={if allow_download { "" } else { "display:none" }}>
                        <a href=format!("/share/{}/download", share_key) rel="external" title="Download all">
                            <img src="/images/download-all.svg" alt="" />
                            <span>"Download all"</span>
                        </a>
                    </div>
                </div>
            </div>
            {album_description.map(|desc| {
                view! {
                    <div id="album-description">
                        <h2>{desc}</h2>
                    </div>
                }
            })}

            <div id="lightgallery">
                <For
                    each=move || assets.clone().into_iter().enumerate()
                    key=|(_, a)| a.id.clone()
                    children={
                        let s_key_for_tile = share_key.clone();
                        move |(i, asset)| {
                            view! {
                                <AssetTile
                                    i=i
                                    asset=asset
                                    share_key=s_key_for_tile.clone()
                                    selected_assets=selected_assets
                                    on_toggle=on_toggle_select
                                />
                            }
                        }
                    }
                />
            </div>

            <script inner_html=gallery_data />
            <script>
                "window.initLG = () => {
                    if (window.lgallery && window.GALLERY_DATA) {
                        window.lgallery.init(window.GALLERY_DATA);
                    }
                };
                if (document.readyState === 'complete') window.initLG();
                else document.addEventListener('DOMContentLoaded', window.initLG);"
            </script>
        </div>
    }
}
