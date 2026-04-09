# Immich Public Proxy (Rust Edition)

[![Release](https://img.shields.io/github/v/release/daniel5151/immich-public-proxy-rs?style=flat-square)](https://github.com/daniel5151/immich-public-proxy-rs/releases)
[![License](https://img.shields.io/github/license/daniel5151/immich-public-proxy-rs?style=flat-square)](LICENSE)

A Rust rewrite (and extension) of [alangrainger/immich-public-proxy](https://github.com/alangrainger/immich-public-proxy).

Share photos and albums from [Immich](https://github.com/immich-app/immich) without exposing your entire instance or requiring API keys for public access.

---

## Features

- **Stateless Proxy**: Does not keep local state; validates requests against Immich's share IDs/slugs.
- **Media Support**: Proxies both images and videos. Video support includes range requests for seeking and playback compatibility.
- **Password Support**: Supports Immich's native password-protected shares using session cookies.
- **Download Management**:
    - **Download All**: Allows downloading the entire share as a ZIP archive.
    - **Selection-based Downloads**: Users can select specific assets in the gallery to download as a custom ZIP.
- **Upload Support**: Users can upload media directly to shared albums via the proxy.
    - NOTE: at the moment, uploads are attributed to the user who created the share. In the future, this will be tweaked to upload images with an uploader-defined tag of `SharedBy/{name}` instead, and/or uploading files under a dedicated "Public-Uploader" service account.
- **Gallery UI**:
    - Chronological grouping of assets by date.
    - Lazy loading of grid tiles using `IntersectionObserver` for performance in large albums.
    - Responsive layout for desktop and mobile browsers.
    - Render Uploader Attribution Badges (pulled from `SharedBy/{name}` tags).
- **SEO/Metadata**: Server-side rendering (SSR) provides OpenGraph meta tags for link previews.

---

## Comparison with Upstream

This project implements the core functionality of the original Node.js proxy with a different architecture.

### Differences in this Implementation (Rust)

| Feature                   | Details                                                                                          |
| :------------------------ | :----------------------------------------------------------------------------------------------- |
| **Server-Side Rendering** | Uses Leptos for SSR, allowing for SEO-friendly link previews without client-side-only rendering. |
| **Bulk Selection**        | Native UI for selecting and downloading a subset of assets as a ZIP.                             |
| **Lazy Loading**          | Explicit `IntersectionObserver` implementation for large grids.                                  |
| **Single Binary**         | Compiles to a single binary for easier deployment outside of Docker.                             |

### Upstream Features Not Currently Implemented

In a nutshell: quite a bit!

The current implementation is tailored to my personal preferences and needs, and I haven't added features / config machinery outside of what I personally use.

That said, I'm not opposed to adding more features + config machinery if there's interest from folks.

---

## Installation

### Manual Build
1. Install the Rust toolchain and `wasm32-unknown-unknown` target.
2. Install `cargo-leptos`.
3. Build the project:
   ```bash
   cargo leptos build --release
   ```
4. Deploy the following files to your server:
   - Server binary: `target/release/immich-public-proxy-rs`
   - Site assets: `target/site/`


### Proxmox VE
If you are running Proxmox, I've got a forked version of the upstream community script that you can use to install this Rust version.

Run this _inside_ your Immich Public Proxy LXC container.

```bash
bash -c "$(wget -qLO - https://raw.githubusercontent.com/daniel5151/immich-public-proxy-rs/main/scripts/community-scripts-ProxmoxVE/immich-public-proxy-rs.sh)"
```

---

## Configuration

Configuration is handled via environment variables.

| Variable             | Required | Description                                                                  |
| :------------------- | :------: | :--------------------------------------------------------------------------- |
| `IMMICH_URL`         |   Yes    | The internal URL of your Immich instance (e.g., `http://192.168.1.50:2283`). |
| `LEPTOS_SITE_ROOT`   |   Yes    | Path to the `site` directory containing static assets.                       |
| `LEPTOS_SITE_ADDR`   |   Yes    | Address and port to bind to.                                                 |
| `LEPTOS_OUTPUT_NAME` |   Yes    | The name of the compiled project.                                            |
| `IMMICH_API_KEY`     |    No    | API key of the shared album owner (permissions determine proxy features)     |
| `RUST_LOG`           |    No    | Logging level (e.g., `info`, `debug`, `warn`).                               |

### `IMMICH_API_KEY` Features and Permissions

Certain features require configuring an `IMMICH_API_KEY` with the appropriate permissions.

NOTE: At the moment, only a single API key is supported, corresponding to the album share owner. In the future, support for multiple API keys (to allow multiple users to use the proxy) may be added.

| Feature                         | Required Permissions     | Notes
| :------------------------------ | :----------------------- | -----
| **Password Protected Shares**   | `sharedLink.read`        | Used to query if a passwd is required for a share
| **Uploader Attribution Badges** | `tag.read`, `asset.read` | Relies on photos having tags with format `SharedBy/{name}`

### Systemd Service Example

```ini
[Unit]
Description=Immich Public Proxy
After=network.target

[Service]
Type=simple
User=immich
WorkingDirectory=/opt/immich-public-proxy-rs
# Option 1: Use an environment file
EnvironmentFile=/opt/immich-public-proxy-rs/.env
# Option 2: Set variables directly
# Environment=IMMICH_URL=http://192.168.1.10:2283
# Environment=LEPTOS_SITE_ROOT=site
# Environment=LEPTOS_SITE_ADDR=0.0.0.0:3000
ExecStart=/opt/immich-public-proxy-rs/immich-public-proxy-rs
Restart=always

[Install]
WantedBy=multi-user.target
```

---

## License

This project is licensed under the GNU Affero General Public License v3.0. See [LICENSE](LICENSE) for details.
