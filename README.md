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
    - Uploaded assets are processed in the background, added to the shared album, and tagged with `SharedBy/{uploader_name}` using the configured upload service account API key (`IMMICH_API_KEY_UPLOAD_USER`).
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
| **Server-Side Rendering** | Rust Axum backend injects SEO/OpenGraph previews dynamically, then mounts a decoupled React SPA. |
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
1. Install the Rust toolchain and Node.js (v20+).
2. Build the project:
   ```bash
   (cd frontend && npm install && npm run build)
   cargo build --release
   ```
3. Deploy the following files to your server:
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

| Variable                     | Required | Description                                                                                                                                           |
| :--------------------------- | :------: | :---------------------------------------------------------------------------------------------------------------------------------------------------- |
| `IMMICH_URL`                 |   Yes    | The internal URL of your Immich instance (e.g., `http://192.168.1.50:2283`).                                                                          |
| `LEPTOS_SITE_ROOT`           |    No    | Path to the static assets directory. Defaults to `target/site`.                                                                                       |
| `LEPTOS_SITE_ADDR`           |    No    | Address and port to bind to. Defaults to `127.0.0.1:3000`.                                                                                            |
| `IMMICH_API_KEY`             |    No    | Admin/owner API key. Enables password detection, link-not-found resolution, and name fallback resolution.                                             |
| `IMMICH_API_KEY_UPLOAD_USER` |    No    | Service account/user API key for uploads. Enabling upload support requires this key.                                                                  |
| `PUBLIC_BASE_URL`            |    No    | The public URL of the proxy (e.g., `https://photos.example.com`). Used for SEO/OpenGraph preview tags. Generated dynamically from headers if omitted. |
| `RUST_LOG`                   |    No    | Logging level (e.g., `info`, `debug`, `warn`). Defaults to `error`.                                                                                   |

### API Key Permissions and Features

Depending on the features you enable, your API keys require specific permissions in Immich.

#### `IMMICH_API_KEY` (Owner/Admin API Key)

Used to fetch share details, check passwords, and display uploader attribution badges.

| Feature                         | Required Permissions     | Notes                                                                                                             |
| :------------------------------ | :----------------------- | ----------------------------------------------------------------------------------------------------------------- |
| **Password Protected Shares**   | `sharedLink.read`        | Used to query if a password is required for a share.                                                              |
| **Link Not Found Resolution**   | `sharedLink.read`        | Distinguishes invalid keys from password-protected keys, avoiding infinite password prompt loops.                 |
| **Uploader Attribution Badges** | `tag.read`, `asset.read` | Relies on photos having tags with format `SharedBy/{name}`.                                                        |
| **Efficient User Resolution**   | `user.read`              | Recommended; allows fetching all users at once to resolve owner fallback names, avoiding slow sequential requests. |

#### `IMMICH_API_KEY_UPLOAD_USER` (Upload Service Account API Key)

Required to support uploading to shared albums.

| Feature                        | Required Permissions            | Notes                                                                                            |
| :----------------------------- | :------------------------------ | :----------------------------------------------------------------------------------------------- |
| **Asset Upload**               | `asset.create`                  | Required to upload photos and videos to Immich.                                                  |
| **Uploader Tagging**           | `tag.create`, `tag.read`        | Required to create and apply `SharedBy/{name}` tags.                                             |
| **Album Association**          | `album.write` or `album.update` | Required to add the uploaded asset to the shared album.                                          |
| **Restore Trashed Duplicates** | `asset.delete`                  | Required if you want the proxy to automatically restore duplicate uploads that were manually trashed. |

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

## Dev Flow

You'll need to run two processes simultaneously.

```bash
(cd frontend && npx vite build --watch)
```

```bash
IMMICH_URL=http://<immich-ip>:2283 \
IMMICH_API_KEY="..." \
IMMICH_API_KEY_UPLOAD_USER="..." \
cargo run
```

---

## License

This project is licensed under the GNU Affero General Public License v3.0. See [LICENSE](LICENSE) for details.
