use crate::api::get_share_details::{ShareDetails, get_share_details};
use leptos::prelude::*;
use leptos_meta::{Body, Meta, Stylesheet, Title, provide_meta_context};
use leptos_router::{
    ParamSegment, StaticSegment,
    components::{Route, Router, Routes},
};
use std::collections::HashSet;

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <Stylesheet id="leptos" href="/pkg/immich-public-proxy-rs.css"/>
        <script src="/web.js"/>

        <Stylesheet href="/lg/lightgallery-bundle.min.css"/>
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
                    <Route path=(StaticSegment("s"), ParamSegment("key")) view=SharePage ssr=leptos_router::SsrMode::Async/>
                    <Route path=(StaticSegment("share"), ParamSegment("key")) view=SharePage ssr=leptos_router::SsrMode::Async/>
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
    let key = move || params.with(|p| p.get("key").expect("key must be present"));

    let share_res = Resource::new(key, |k| get_share_details(k, None));

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
                <form id="unlock" method="post" action="/share/unlock" on:submit=move |ev| { ev.stop_propagation(); } style="display:flex;flex-direction:column;gap:1rem;">
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
    asset: crate::immich_client::model::Asset,
    share_key: (String, String),
    selected_assets: RwSignal<HashSet<String>>,
    on_toggle: ipp_callback::Callback<String>,
) -> impl IntoView {
    let id = asset.id.clone();
    let id_for_selected = id.clone();
    let id_for_toggle = id.clone();

    let thumbnail_url = if share_key.0 == share_key.1 {
        format!("/share/photo/{}/{}/thumbnail", share_key.0, id)
    } else {
        format!(
            "/share/photo/{}/{}/thumbnail?sk={}",
            share_key.0, id, share_key.1
        )
    };
    let is_video = asset.r#type == "VIDEO";

    let width = match asset.width {
        Some(w) => w as f32,
        None => {
            return view! { <div class="error-msg">"Error: asset width must be present"</div> }
                .into_any();
        }
    };
    let height = match asset.height {
        Some(h) => h as f32,
        None => {
            return view! { <div class="error-msg">"Error: asset height must be present"</div> }
                .into_any();
        }
    };
    let aspect_ratio = width / height;
    let flex_basis = format!("{}px", 250.0 * aspect_ratio);

    let is_selected = move || selected_assets.get().contains(&id_for_selected);
    let preview_url = if share_key.0 == share_key.1 {
        format!("/share/photo/{}/{}/preview", share_key.0, id)
    } else {
        format!(
            "/share/photo/{}/{}/preview?sk={}",
            share_key.0, id, share_key.1
        )
    };

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
            >
                <span class="check-icon"></span>
            </div>
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
    .into_any()
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
fn Gallery(details: ShareDetails) -> impl IntoView {
    let link = details.link;
    let real_key = link.key.clone();
    let request_key = details.request_key.clone();
    let share_key = (real_key.clone(), request_key.clone());
    let assets = link.assets.clone();
    let allow_download = match link.allow_download {
        Some(a) => a,
        None => {
            return view! { <div class="error-msg">"Error: allow_download must be present"</div> }
                .into_any();
        }
    };

    let title = match link
        .description
        .clone()
        .or_else(|| link.album.as_ref().and_then(|a| a.album_name.clone()))
    {
        Some(t) => t,
        None => return view! { <div class="error-msg">"Error: gallery title/description must be present"</div> }.into_any(),
    };

    let album_description = link.album.as_ref().and_then(|a| a.description.clone());
    let public_base_url = details.public_base_url.trim_end_matches('/').to_string();
    let current_url = format!("{}/share/{}", public_base_url, request_key);
    let cover_image_url = assets
        .first()
        .map(|a| {
            if real_key == request_key {
                format!(
                    "{}/share/photo/{}/{}/preview",
                    public_base_url, real_key, a.id
                )
            } else {
                format!(
                    "{}/share/photo/{}/{}/preview?sk={}",
                    public_base_url, real_key, a.id, request_key
                )
            }
        })
        .unwrap_or_default();

    let selected_assets = RwSignal::new(HashSet::<String>::new());

    #[derive(Clone)]
    struct AssetGroup {
        label: String,
        items: Vec<(usize, crate::immich_client::model::Asset)>,
    }

    let mut groups: Vec<AssetGroup> = Vec::new();
    for (i, asset) in assets.iter().enumerate() {
        let date_label = match &asset.file_created_at {
            Some(dstr) => {
                let parsed: Option<chrono::DateTime<chrono::FixedOffset>> =
                    chrono::DateTime::parse_from_rfc3339(dstr).ok();
                match parsed {
                    Some(dt) => dt.format("%a, %b %-d, %Y").to_string(),
                    None => "Unknown Date".to_string(),
                }
            }
            None => "Unknown Date".to_string(),
        };

        if let Some(last) = groups.last_mut() {
            if last.label == date_label {
                last.items.push((i, asset.clone()));
                continue;
            }
        }
        groups.push(AssetGroup {
            label: date_label,
            items: vec![(i, asset.clone())],
        });
    }

    // LightGallery items array format for dynamic mode
    let items_array = assets
        .iter()
        .map(|asset| {
            let key = real_key.clone();
            let sk = request_key.clone();
            let (preview_url, thumbnail_url, download_url) = if key == sk {
                (
                    format!("/share/photo/{}/{}/preview", key, asset.id),
                    format!("/share/photo/{}/{}/thumbnail", key, asset.id),
                    format!("/share/photo/{}/{}/original", key, asset.id),
                )
            } else {
                (
                    format!("/share/photo/{}/{}/preview?sk={}", key, asset.id, sk),
                    format!("/share/photo/{}/{}/thumbnail?sk={}", key, asset.id, sk),
                    format!("/share/photo/{}/{}/original?sk={}", key, asset.id, sk),
                )
            };

            if asset.r#type == "VIDEO" {
                serde_json::json!({
                    "video": {
                        "source": [
                            {
                                "src": if key == sk {
                                    format!("/share/video/{}/{}", key, asset.id)
                                } else {
                                    format!("/share/video/{}/{}?sk={}", key, asset.id, sk)
                                },
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
            if s_key_for_download.0 == s_key_for_download.1 {
                format!(
                    "/share/{}/download?asset_ids={}",
                    s_key_for_download.0, ids_str
                )
            } else {
                format!(
                    "/share/{}/download?asset_ids={}&sk={}",
                    s_key_for_download.0, ids_str, s_key_for_download.1
                )
            }
        }
    };

    let is_selection_mode = move || !selected_assets.get().is_empty();

    view! {
        <Title text=title.clone() />
        <Meta name="og:title" content=title.clone() />
        <Meta name="twitter:title" content=title.clone() />
        <Meta name="description" content=album_description.clone().unwrap_or_default() />
        <Meta name="og:description" content=album_description.clone().unwrap_or_default() />
        <Meta name="twitter:description" content=album_description.clone().unwrap_or_default() />
        <Meta name="og:image" content=cover_image_url.clone() />
        <Meta name="twitter:image" content=cover_image_url.clone() />
        <Meta name="twitter:card" content="summary_large_image".to_string() />

        <Meta name="og:url" content=current_url.clone() />

        <Body attr:class=move || if is_selection_mode() { "selection-mode" } else { "" } />
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
                        <a href=if share_key.0 == share_key.1 {
                            format!("/share/{}/download", share_key.0)
                        } else {
                            format!("/share/{}/download?sk={}", share_key.0, share_key.1)
                        } rel="external" title="Download all">
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
                    each=move || groups.clone().into_iter()
                    key=|g| g.label.clone()
                    children=move |group| {
                        let label = group.label.clone();
                        let group_items = group.items.clone();
                        let s_key_for_tile = share_key.clone();

                        let has_all_selected = {
                            let items = group_items.clone();
                            move || {
                                let selected = selected_assets.get();
                                items.iter().all(|(_, a)| selected.contains(&a.id))
                            }
                        };

                        let on_group_toggle = {
                            let items = group_items.clone();
                            move |_| {
                                let is_all_selected = selected_assets.with(|set| {
                                    items.iter().all(|(_, a)| set.contains(&a.id))
                                });

                                selected_assets.update(|set| {
                                    if is_all_selected {
                                        for (_, a) in &items {
                                            set.remove(&a.id);
                                        }
                                    } else {
                                        for (_, a) in &items {
                                            set.insert(a.id.clone());
                                        }
                                    }
                                });
                            }
                        };

                        view! {
                            <div class="gallery-date-group">
                                <div class="gallery-date-header">
                                    <span class="date-label">{label}</span>
                                    <div
                                        class="date-selector"
                                        class:selected=has_all_selected
                                        on:click=on_group_toggle
                                    >
                                        <span class="check-icon"></span>
                                    </div>
                                </div>
                                <div class="gallery-date-items">
                                    <For
                                        each=move || group_items.clone()
                                        key=|(_, a)| a.id.clone()
                                        children={
                                            let key_clone = s_key_for_tile.clone();
                                            move |(i, asset)| {
                                                view! {
                                                    <AssetTile
                                                        i=i
                                                        asset=asset
                                                        share_key=key_clone.clone()
                                                        selected_assets=selected_assets
                                                        on_toggle=on_toggle_select
                                                    />
                                                }
                                            }
                                        }
                                    />
                                </div>
                            </div>
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
    }.into_any()
}
