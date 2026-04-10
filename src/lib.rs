cfg_if::cfg_if! {
    if #[cfg(feature = "hydrate")] {
        mod api;
        mod app;
        mod dto;
        mod immich_client;

        #[wasm_bindgen::prelude::wasm_bindgen]
        pub fn hydrate() {
            use crate::app::*;
            console_error_panic_hook::set_once();
            leptos::mount::hydrate_body(App);
        }

    }
}
