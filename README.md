# immich-public-proxy-rs

Rust rewrite of https://github.com/alangrainger/immich-public-proxy

## Executing a Server on a Remote Machine Without the Toolchain

After running a `cargo leptos build --release` the minimum files needed are:

1. The server binary located in `target/release`
2. The `site` directory and all files within located in `target/site`

Copy these files to your remote server. The directory structure should be:
```text
immich-public-proxy-rs
site/
```

Set the following environment variables (updating for your project as needed):
```sh
export LEPTOS_OUTPUT_NAME="immich-public-proxy-rs"
export LEPTOS_SITE_ROOT="site"
export LEPTOS_SITE_PKG_DIR="pkg"
export LEPTOS_SITE_ADDR="127.0.0.1:3000"
export LEPTOS_RELOAD_PORT="3001"
```

Finally, run the server binary.
