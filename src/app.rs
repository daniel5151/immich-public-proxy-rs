use crate::api::get_share_details::ShareDetails;
use crate::api::get_share_details::get_share_details;
use leptos::prelude::*;
use leptos_meta::Body;
use leptos_meta::Meta;
use leptos_meta::Stylesheet;
use leptos_meta::Title;
use leptos_meta::provide_meta_context;
use leptos_router::ParamSegment;
use leptos_router::StaticSegment;
use leptos_router::components::Route;
use leptos_router::components::Router;
use leptos_router::components::Routes;
use std::collections::HashSet;
#[cfg(target_arch = "wasm32")]
use web_sys::{FormData, HtmlInputElement, Request, RequestInit};

#[derive(Clone)]
#[allow(dead_code)]
enum UploadResult {
    Success,
    Failed(String),
}

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
    share_key: String,
    selected_assets: RwSignal<HashSet<String>>,
    on_toggle: Callback<String>,
) -> impl IntoView {
    let id = asset.id.clone();
    let id_for_selected = id.clone();
    let id_for_toggle = id.clone();

    let thumbnail_url = format!("/share/photo/{}/{}/thumbnail", share_key, id);
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

#[component]
fn Gallery(details: ShareDetails) -> impl IntoView {
    let link = details.link;

    let real_key = link.key.clone();
    let request_key = details.request_key.clone();

    let assets = link.assets.clone();
    let allow_download = match link.allow_download {
        Some(a) => a,
        None => {
            return view! { <div class="error-msg">"Error: allow_download must be present"</div> }
                .into_any();
        }
    };

    let allow_upload = link.allow_upload.unwrap_or(false);

    let title: String = match link
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
    let cover_asset_id = link
        .album
        .as_ref()
        .and_then(|a| a.album_thumbnail_asset_id.clone())
        .or_else(|| assets.first().map(|a| a.id.clone()));

    let cover_image_url = cover_asset_id
        .map(|id| {
            format!(
                "{}/share/photo/{}/{}/preview",
                public_base_url, real_key, id
            )
        })
        .unwrap_or_default();

    let selected_assets = RwSignal::new(HashSet::<String>::new());

    let total_assets = assets.len();
    let display_count = RwSignal::new(std::cmp::min(40, total_assets));
    let is_loading_more = RwSignal::new(false);
    let has_observed = RwSignal::new(false);

    let is_uploading = RwSignal::new(false);
    let upload_progress = RwSignal::new((0, 0)); // (completed, total)
    let upload_status = RwSignal::new(Option::<UploadResult>::None);

    let on_upload_change = {
        let real_key_signal = RwSignal::new(real_key.clone());
        move |ev: leptos::ev::Event| {
            cfg_if::cfg_if! {
                if #[cfg(target_arch = "wasm32")] {
                    use wasm_bindgen::JsCast;
                    let Some(target) = ev.target() else { return };
                    let input = target.unchecked_into::<HtmlInputElement>();
                    let files = input.files();
                    let Some(file_list) = files else { return };
                    let count = file_list.length();
                    if count == 0 { return; }

                    is_uploading.set(true);
                    upload_progress.set((0, count as usize));
                    upload_status.set(None);

                    let real_key = real_key_signal.get();

                    leptos::task::spawn_local(async move {
                        let mut success = true;
                        let mut failed_name = String::new();

                        for i in 0..count {
                            let Some(file) = file_list.item(i) else { continue };
                            let file_name = file.name();

                            let form_data = FormData::new().unwrap();
                            let file_date = js_sys::Reflect::get(&file, &"lastModified".into())
                                .ok()
                                .and_then(|v| v.as_f64())
                                .and_then(|ms| chrono::DateTime::from_timestamp_millis(ms as i64))
                                .unwrap_or_else(chrono::Utc::now)
                                .to_rfc3339();

                            let blob = file.unchecked_ref::<web_sys::Blob>();
                            form_data.append_with_blob_and_filename("assetData", blob, &file_name).unwrap();
                            form_data.append_with_str("deviceAssetId", &file_name).unwrap();
                            form_data.append_with_str("deviceId", "immich-public-proxy").unwrap();
                            form_data.append_with_str("fileCreatedAt", &file_date).unwrap();
                            form_data.append_with_str("fileModifiedAt", &file_date).unwrap();

                            let opts = RequestInit::new();
                            opts.set_method("POST");
                            opts.set_body(&form_data);

                            let url = format!("/share/{}/upload", real_key);
                            let request = Request::new_with_str_and_init(&url, &opts).unwrap();

                            let window = web_sys::window().unwrap();
                            let resp_value: Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue> =
                                wasm_bindgen_futures::JsFuture::from(window.fetch_with_request(&request)).await;

                            match resp_value {
                                Ok(v) => {
                                    let resp: web_sys::Response = v.unchecked_into();
                                    if !resp.ok() {
                                        success = false;
                                        failed_name = file_name.clone();
                                        break;
                                    }
                                }
                                Err(_) => {
                                    success = false;
                                    failed_name = file_name.clone();
                                    break;
                                }
                            }

                            upload_progress.update(|p| p.0 += 1);
                        }

                        if success {
                            upload_status.set(Some(UploadResult::Success));
                        } else {
                            upload_status.set(Some(UploadResult::Failed(failed_name)));
                        }
                        is_uploading.set(false);
                    });
                } else {
                    let _ = ev;
                    let _ = real_key_signal;
                }
            }
        }
    };

    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::JsCast;
        use wasm_bindgen::closure::Closure;
        use web_sys::IntersectionObserver;
        use web_sys::IntersectionObserverEntry;
        use web_sys::IntersectionObserverInit;

        let is_intersecting = RwSignal::new(false);

        let closure = Closure::wrap(Box::new(
            move |entries: js_sys::Array, _observer: IntersectionObserver| {
                for entry in entries.iter() {
                    let entry: IntersectionObserverEntry = entry.unchecked_into();
                    is_intersecting.set(entry.is_intersecting());
                }
                has_observed.set(true);
            },
        )
            as Box<dyn FnMut(js_sys::Array, IntersectionObserver)>);

        Effect::new(move |_| {
            if is_intersecting.get() {
                let current = display_count.get();
                if current < total_assets && !is_loading_more.get_untracked() {
                    is_loading_more.set(true);
                    let next = std::cmp::min(current + 10, total_assets);

                    set_timeout(
                        move || {
                            is_loading_more.set(false);
                            display_count.set(next);
                        },
                        std::time::Duration::from_millis(400),
                    );
                }
            }
        });

        if let Some(window) = web_sys::window() {
            if let Some(document) = window.document() {
                // Initialize observer only once during hydrate mounting cycle
                set_timeout(
                    move || {
                        if let Some(target) = document.get_element_by_id("loading-observer") {
                            let options = IntersectionObserverInit::new();
                            options.set_root_margin("1000px 0px 1000px 0px"); // margin to eagerly start loading items offscreen

                            if let Ok(observer) = IntersectionObserver::new_with_options(
                                closure.as_ref().unchecked_ref(),
                                &options,
                            ) {
                                observer.observe(&target);
                                closure.forget();
                            }
                        }
                    },
                    std::time::Duration::from_millis(100),
                );
            }
        }
    }

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

    let on_toggle_select = Callback::new(move |id: String| {
        selected_assets.update(|set| {
            if set.contains(&id) {
                set.remove(&id);
            } else {
                set.insert(id);
            }
        });
    });

    let s_key_for_download = real_key.clone();
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

    let is_selection_mode = move || !selected_assets.get().is_empty();
    let real_key_header = real_key.clone();

    view! {
        <Title text=title.clone() />
        <Meta name="description" content=album_description.clone().unwrap_or_default() />
        <Meta name="og:description" content=album_description.clone().unwrap_or_default() />
        <Meta name="og:image" content=cover_image_url.clone() />
        <Meta name="og:title" content=title.clone() />
        <Meta name="twitter:card" content="summary_large_image".to_string() />
        <Meta name="twitter:description" content=album_description.clone().unwrap_or_default() />
        <Meta name="twitter:image" content=cover_image_url.clone() />
        <Meta name="twitter:title" content=title.clone() />

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
                    <Show when=move || allow_upload>
                        <div id="upload-action">
                            <label class={move || if is_uploading.get() { "header-btn disabled" } else { "header-btn" }}>
                                <img src="/images/align-top-svgrepo-com.svg" alt="" class="header-icon" />
                                <span>"Upload"</span>
                                <input type="file" multiple accept="image/*,video/*" class="hidden-file-input" disabled=move || is_uploading.get() on:change=on_upload_change />
                            </label>
                        </div>
                    </Show>
                    <Show when=move || allow_download>
                        <div id="download-all">
                            <a href=format!("/share/{}/download", real_key_header) rel="external" title="Download all" class="header-btn">
                                <img src="/images/align-bottom-svgrepo-com.svg" alt="" class="header-icon" />
                                <span>"Download all"</span>
                            </a>
                        </div>
                    </Show>
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
                        let group_start_index = group.items.first().unwrap().0;
                        let is_visible = move || display_count.get() > group_start_index;
                        let real_key_clone = real_key.clone();

                        view! {
                            <Show when=is_visible>
                                {
                                    let label = group.label.clone();
                                    let group_items = group.items.clone();
                                    let s_key_for_tile = real_key_clone.clone();

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
                                            each={
                                                let items = group_items.clone();
                                                move || {
                                                    let current = display_count.get();
                                                    items.iter().filter(|(i, _)| *i < current).cloned().collect::<Vec<_>>()
                                                }
                                            }
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
                            </Show>
                        }
                    }
                />
            </div>

            <div id="loading-observer" style="height: 1px; width: 100%;"></div>
            <Show when=move || {
                // If we haven't received initial observation yet, fallback to whether we have more items to load
                if !has_observed.get() {
                    display_count.get() < total_assets
                } else {
                    is_loading_more.get()
                }
            }>
                <div id="loading-spinner">
                    <span class="loader"></span>
                </div>
            </Show>

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

            <Show when=move || is_uploading.get() || upload_status.get().is_some()>
                <div id="upload-toast">
                    <Show when=move || is_uploading.get()>
                        <div class="toast-content uploading">
                            <span class="loader-small"></span>
                            <span>"Uploading " {move || upload_progress.get().0} "/" {move || upload_progress.get().1}</span>
                        </div>
                    </Show>
                    <Show when=move || !is_uploading.get() && matches!(upload_status.get(), Some(UploadResult::Success))>
                        <div class="toast-content success">
                            <div style="display:flex;flex-direction:column;gap:4px">
                                <span>"✅ Upload complete"</span>
                                <span style="font-size:0.8rem;opacity:0.75">"Reload the page to see your new photos."</span>
                            </div>
                        </div>
                    </Show>
                    <Show when=move || !is_uploading.get() && matches!(upload_status.get(), Some(UploadResult::Failed(_)))>
                        <div class="toast-content failed">
                            <span>"❌ Failed to upload: " {move || match upload_status.get() {
                                Some(UploadResult::Failed(name)) => name,
                                _ => String::new(),
                            }}</span>
                        </div>
                    </Show>
                </div>
            </Show>
        </div>
    }.into_any()
}
