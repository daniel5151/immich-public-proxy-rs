use leptos::prelude::*;
use leptos_meta::{provide_meta_context, MetaTags, Stylesheet, Title};
use leptos_router::{
    components::{Route, Router, Routes},
    ParamSegment, StaticSegment,
};

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

    // We can fetch info from the server using the server function
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
        <Stylesheet href="/pico.min.css"/>
        <main class="container">
            <div class="grid">
                <div></div>
                <div>
                    <form id="unlock" method="post" action="/share/unlock">
                        <input type="password" name="password" placeholder="Password" aria-label="Password" required autofocus />
                        <input type="hidden" name="key" value=required_key.clone() />
                        <button type="submit">"Unlock"</button>
                    </form>
                </div>
                <div></div>
            </div>
        </main>
    }
}

#[component]
fn Gallery(details: crate::server_fns::ShareDetails) -> impl IntoView {
    let link = details.link;
    let title = link
        .description
        .clone()
        .or_else(|| link.album.as_ref().and_then(|a| a.album_name.clone()))
        .unwrap_or_else(|| "Gallery".to_string());

    let album_description = link.album.as_ref().and_then(|a| a.description.clone());
    let allow_download = link.allow_download.unwrap_or(true);

    // We need to build the items array for lgallery.init
    let items_array = link.assets.iter().map(|asset| {
        let key = link.key.clone();
        let preview_url = format!("/share/photo/{}/{}/preview", key, asset.id);
        let thumbnail_url = format!("/share/photo/{}/{}/thumbnail", key, asset.id);
        let mut download_url = format!("/share/photo/{}/{}/original", key, asset.id);
        let mut video = serde_json::Value::Null;

        let item_description = asset.exif_info.as_ref()
            .and_then(|exif| exif.get("description"))
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string();

        if asset.r#type == "VIDEO" {
            download_url = format!("/share/video/{}/{}", key, asset.id);
            video = serde_json::json!({
                "source": [
                    {
                        "src": format!("/share/video/{}/{}", key, asset.id),
                        "type": asset.original_mime_type.clone().unwrap_or_else(|| "video/mp4".to_string())
                    }
                ],
                "attributes": {
                    "playsinline": "playsinline",
                    "controls": "controls"
                }
            });
        }

        let mut html = format!("<a href=\"{}\"", preview_url);
        if asset.r#type == "VIDEO" {
            html = format!("<a data-video='{}'", serde_json::to_string(&video).unwrap().replace("'", "&apos;"));
        }
        html += &format!(" data-download-url=\"{}\"", download_url);
        if !item_description.is_empty() {
            html += &format!(" data-sub-html=\"<p>{}</p>\"", item_description.replace("\"", "&quot;"));
        }
        html += &format!(" data-download=\"{}\" data-slide-name=\"{}\">", asset.original_file_name.clone().unwrap_or_else(|| asset.id.clone()), asset.id);
        html += &format!("<img alt=\"{}\" loading=\"lazy\" src=\"{}\" onerror=\"this.closest('a').classList.add('thumb-error')\" />", item_description.replace("\"", "&quot;"), thumbnail_url);
        if asset.r#type == "VIDEO" {
            html += "<div class=\"play-icon\"></div>";
        }
        html += "</a>";

        serde_json::json!({
            "html": html,
            "thumbnailUrl": thumbnail_url,
            "previewUrl": preview_url
        })
    }).collect::<Vec<_>>();

    let items_json = serde_json::to_string(&items_array).unwrap();
    let assets_len = link.assets.len();

    let gallery_data = format!(
        "window.GALLERY_DATA = {{ lgConfig: {{ controls: true, download: true, customSlideName: true, mobileSettings: {{ controls: false, showCloseIcon: true, download: true }} }}, items: {} }};",
        items_json
    );

    view! {
        <div id="gallery-root">
            <div id="header">
                <h1>{title}</h1>
                <div id="download-all" style={if allow_download { "" } else { "display:none" }}>
                    <a href=format!("/share/{}/download", link.key) title="Download all">
                        <img src="/images/download-all.svg" height="24" width="24" alt="Download all" />
                    </a>
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
                {link.assets.into_iter().take(50).map(|asset| {
                    let key = link.key.clone();
                    let preview_url = format!("/share/photo/{}/{}/preview", key, asset.id);
                    let thumbnail_url = format!("/share/photo/{}/{}/thumbnail", key, asset.id);

                    let item_description = asset.exif_info.as_ref()
                        .and_then(|exif| exif.get("description"))
                        .and_then(|d| d.as_str())
                        .unwrap_or("")
                        .to_string();

                    let is_video = asset.r#type == "VIDEO";
                    let video_attr = if is_video {
                        let video = serde_json::json!({
                            "source": [
                                {
                                    "src": format!("/share/video/{}/{}", key, asset.id),
                                    "type": asset.original_mime_type.clone().unwrap_or_else(|| "video/mp4".to_string())
                                }
                            ],
                            "attributes": {
                                "playsinline": "playsinline",
                                "controls": "controls"
                            }
                        });
                        serde_json::to_string(&video).unwrap()
                    } else {
                        "".to_string()
                    };

                    let download_url = if is_video {
                        format!("/share/video/{}/{}", key, asset.id)
                    } else {
                        format!("/share/photo/{}/{}/original", key, asset.id)
                    };
                    let filename = asset.original_file_name.clone().unwrap_or_else(|| asset.id.clone());

                    let item_description_alt = item_description.clone();
                    view! {
                        <a href={if is_video { None } else { Some(preview_url) }} rel="external" data-video={if is_video { Some(video_attr) } else { None }} data-download-url=download_url data-download=filename data-slide-name=asset.id data-sub-html={if !item_description.is_empty() { Some(format!("<p>{}</p>", item_description)) } else { None }}>
                            <img loading="lazy" src=thumbnail_url alt=item_description_alt />
                            {if is_video { view!{<div class="play-icon"></div>}.into_any() } else { view!{<span/>}.into_any() }}
                        </a>
                    }
                }).collect::<Vec<_>>()}
            </div>
            {if assets_len > 50 {
                view! { <div id="loading-spinner"><span class="loader"></span></div> }.into_any()
            } else {
                view! { <span/> }.into_any()
            }}
            <script inner_html=gallery_data />
        </div>
    }
}
