# Project Architecture Guide

This document explains the architecture, directory structure, data lifecycles, and type-safety mechanisms of the **immich-public-proxy-rs** project.

---

## 1. High-Level Architecture

The project consists of a stateless Rust backend (built with **Axum**) and a decoupled Single Page Application (SPA) frontend (built with **React, Vite, and TypeScript**).

The backend acts as a secure reverse proxy to your internal Immich instance. It serves the static frontend assets, handles JSON API requests, proxies media streams (with seeks/range requests), generates dynamic ZIP files for downloads, and injects SEO/OpenGraph preview tags on the fly.

```mermaid
graph TD
    subgraph Client [Browser / Frontend]
        React[React SPA]
        LG[lightGallery Lightbox]
        TS[TypeScript Fetch Client]
    end

    subgraph Backend [Rust / Axum Server]
        Axum[Axum Router]
        Proxy[Media & ZIP Proxy]
        REST[JSON REST Endpoints]
        SEO[SEO Meta Injector]
    end

    subgraph External [Upstream]
        Immich[Immich API Server]
    end

    React -->|API & Media Requests| Axum
    Axum -->|Serves Static Files| React
    Proxy -->|Forward / Stream Media| Immich
    REST -->|Fetch Share Details / Upload| Immich
```

---

## 2. Directory Structure

```
.
├── .github/workflows/          # GitHub actions (automated release builds)
├── assets/                     # Legacy/Original Leptos assets (frozen)
├── deploy.sh                   # Deployment script for copying binaries/assets
├── frontend/                   # Decoupled React frontend
│   ├── public/                 # Static assets (fonts, images, lightGallery)
│   ├── src/
│   │   ├── types/
│   │   │   └── generated/     # Generated TS types (DO NOT edit manually)
│   │   ├── App.tsx             # Core React app code & routing
│   │   ├── index.css           # Modular stylesheet (copied from style/main.css)
│   │   └── main.tsx            # React entry mount point
│   ├── package.json
│   ├── tsconfig.json
│   └── vite.config.ts          # Configured to build into ../target/site
├── src/                        # Rust backend source
│   ├── api/
│   │   ├── get_share_details.rs # Share details endpoint & helper logic
│   │   └── mod.rs
│   ├── immich_client/          # Upstream API client wrappers
│   │   ├── client.rs
│   │   ├── model.rs
│   │   └── mod.rs
│   ├── dto.rs                  # Data Transfer Objects (derived with ts-rs)
│   ├── main.rs                 # Axum server configuration & entry point
│   └── proxy.rs                # Photo/video/upload/download reverse proxy
├── style/                      # Rust styling sources (retained for backward compatibility)
└── target/site/                # Compiled static assets served by Axum
```

---

## 3. Core Lifecycles

### A. Page Load & SEO Meta-Tag Injection
When a request is made for `/share/:key` or `/s/:key`, the Axum backend intercepts the request, queries the metadata from Immich, and injects OpenGraph tags into `index.html` before sending it to the client. This allows rich preview cards in Slack, Discord, etc., without requiring full server-side rendering (SSR) of the SPA.

```mermaid
sequenceDiagram
    participant User as Browser / Bot
    participant Axum as Axum Server
    participant Immich as Immich API

    User->>Axum: GET /share/:key
    Axum->>Immich: Fetch Share Link Details
    alt Valid Share (No Password)
        Immich-->>Axum: Return link meta & assets
        Axum->>Axum: Read target/site/index.html
        Axum->>Axum: Inject <meta property="og:..."/> into <head>
        Axum-->>User: Return SEO-injected HTML
    else Password Protected / Unauthorized
        Immich-->>Axum: Return 401
        Axum->>Axum: Read target/site/index.html (no injection)
        Axum-->>User: Return raw index.html (triggers Password prompt)
    end
    Note over User: React SPA mounts and bootstraps client-side routing
```

### B. Uploading Media
If the share link allows uploading, users can upload photos/videos. The client prompts for an uploader name (saved in `localStorage`), attaches it as a custom header (`x-uploader-name`), and sends files to `/share/:key/upload`. The backend proxies this to Immich using the upload service account API key (`IMMICH_API_KEY_UPLOAD_USER`), adds the assets to the album, and tags them with `SharedBy/{name}` in the background. The background tag/album work runs under a concurrency limit (`IPP_UPLOAD_CONCURRENCY`, default 4) and the share-details cache for the affected key is invalidated so the new asset appears immediately.

**Deferred tag guard.** Immich's asynchronous metadata-extraction job calls `replaceAssetTags()` with the tags embedded in the uploaded file (~1s after upload). Keyword-less files (e.g. `PXL_*.jpg`) carry an empty embedded tag set, so extraction would replace the asset's entire tag set with `[]` — silently wiping the `SharedBy/{name}` attribution the proxy just applied. After the synchronous tag/album step succeeds, the proxy spawns a detached `deferred_tag_guard` that re-checks the asset's tag on a spaced schedule (`IPP_TAG_GUARD_SCHEDULE`, default `2,4,8,16,30`s) straddling the extraction window. If the tag is gone it re-applies it; it only exits after two consecutive confirmations, so it outlasts a delayed wipe. An inconclusive read defers to the next tick rather than blindly re-PUTting (a blind re-apply races Immich into a `tag_asset_pkey` duplicate-key 500). The guard is on by default and can be disabled with `IPP_TAG_GUARD=0`.

```mermaid
sequenceDiagram
    participant User as React Frontend
    participant Axum as Axum Server
    participant Immich as Immich API

    User->>User: Enter Uploader Name (stores in localStorage)
    User->>Axum: POST /share/:key/upload (multipart form + x-uploader-name)
    Axum->>Immich: POST /assets (upload raw data)
    Immich-->>Axum: Return upload metadata
    Axum->>Immich: Create tag 'SharedBy/{name}'
    Axum->>Immich: Apply tag to uploaded asset
    Axum->>Axum: Invalidate share-details cache for key
    Axum-->>User: Return 200 OK / Success
    Note over Axum,Immich: deferred_tag_guard (detached): re-check tag on<br/>2,4,8,16,30s schedule; re-apply if metadata<br/>extraction wiped it; exit after 2 confirmations
```

### C. Upload Status Tracking & SSE Streaming
In a multi-file upload drop, checking the status of each uploaded asset individually (to verify background tagging and album association are complete) would spam the backend with HTTP requests. This can trigger rate limiters (like CrowdSec). The proxy implements two mechanisms to prevent this:

1. **Session-Scoped SSE (Server-Sent Events) Streaming**:
   - At the start of a batch upload, the frontend generates a unique session token using `crypto.randomUUID()`.
   - The frontend opens a single, long-lived EventSource connection to `/share/{key}/status/stream?session=TOKEN`.
   - Each upload POST request includes `?session=TOKEN`. The backend registers the uploaded asset ID in an in-memory session registry (`upload_sessions`) mapping the session token to its pending asset set.
   - The backend stream handler checks the status of these assets. As assets finish processing, it pushes named events (`ready`, `errored`, `done`) back to the frontend.
   - The stream terminates once the client sends a "finish" beacon (`POST /share/{key}/upload/finish?session=TOKEN`) AND all pending assets have finished, or after a 120-second safety backstop timeout.
   - This collapses the status monitoring of large batches to a single HTTP connection.

2. **Batched Status Polling (Fallback)**:
   - If SSE is disabled, the client falls back to batched status polling by sending requests to `/share/{key}/status?ids=a,b,c` at progressive intervals (500ms, 1s, 2s).

```mermaid
sequenceDiagram
    participant User as React Frontend
    participant Axum as Axum Server
    participant Immich as Immich API

    User->>User: Generate session UUID
    User->>Axum: GET /share/:key/status/stream?session=UUID (EventSource)
    Note over Axum: Server opens stream and monitors session registry

    par Uploading files
        User->>Axum: POST /share/:key/upload?session=UUID (File 1)
        Note over Axum: Register File 1 ID in session
        Axum->>Immich: Proxy upload to Immich
        Axum-->>User: Return 200 OK (Asset ID 1)
    and
        User->>Axum: POST /share/:key/upload?session=UUID (File 2)
        Note over Axum: Register File 2 ID in session
        Axum->>Immich: Proxy upload to Immich
        Axum-->>User: Return 200 OK (Asset ID 2)
    end

    Note over Axum,Immich: Backend runs tag & album tasks in background

    loop Stream Status Check
        Axum->>Immich: Fetch asset status (using cached permission check)
        alt Asset 1 Ready
            Axum->>User: SSE event: ready (File 1 SafeAsset JSON)
        end
    end

    User->>Axum: POST /share/:key/upload/finish?session=UUID
    Note over Axum: Mark session as finished uploading

    loop Stream Status Check (remainder)
        alt Asset 2 Ready
            Axum->>User: SSE event: ready (File 2 SafeAsset JSON)
        end
    end

    Note over Axum: All assets processed & finish beacon received
    Axum->>User: SSE event: done (resolved count)
    Note over User,Axum: Connection closed
```

### D. Filter by Uploader (Frontend)
When an album has assets from multiple uploaders (i.e., ≥2 distinct `uploaderName` values), the frontend displays a settings gear button with a unified Settings modal. The modal contains a checkbox filter list showing each uploader's name and photo count (alphabetically sorted). Filtering is applied via `useMemo` before date-grouping and lightGallery index construction, so the lightbox, lazy loading, and grid all operate on the filtered set. Asset selection is independent of the filter — selected assets remain selected even when hidden by the filter. The filter state is ephemeral (not persisted to `localStorage`).

The Settings modal also houses the existing "Uploader Name" input when uploads are enabled, unifying both settings into one panel.

### E. ZIP Downloads
Downloads can be requested for the entire share or a custom selection of checkboxes. The backend handles this by streaming each asset from Immich on-the-fly and wrapping it into a compressed ZIP stream in real-time, avoiding large temporary disk usage.

---

## 4. Shared Type-Safety (Rust $\rightarrow$ TypeScript)

To prevent type drift between the Axum REST API and the React SPA, the project uses compile-time type codegen using **`ts-rs`**.

```
         [Rust Structs] (src/dto.rs)
                │
                │ #[derive(TS)]
                ▼
          (cargo test)
                │
                ▼
   [TypeScript Interfaces] (frontend/src/types/generated/)
                │
                ▼
         [React Client] (frontend/src/App.tsx)
```

### 1. Define Rust Struct
Any payload or model sent over the wire is declared in Rust with the `TS` trait:
```rust
#[derive(Serialize, Deserialize, TS)]
#[ts(export, export_to = "../frontend/src/types/generated/")]
pub struct SafeAsset {
    pub id: String,
    pub original_file_name: Option<String>,
    pub r#type: String, // "IMAGE" or "VIDEO"
    pub original_mime_type: Option<String>,
    pub file_created_at: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub uploader_name: Option<String>,
    #[serde(default)]
    pub uploader_is_fallback: bool,
    #[serde(skip_serializing)]
    #[ts(skip)]
    pub owner_id: Option<String>,
    pub download_url: Option<String>,
}
```

### 2. Export on Test Execution
Running `cargo test` executes the `ts-rs` runner, writing corresponding `.ts` interface declarations:
```typescript
// This file was generated by ts-rs. Do not edit this file manually.

export type SafeAsset = {
  id: string;
  originalFileName: string | null;
  type: string;
  originalMimeType: string | null;
  fileCreatedAt: string | null;
  width: number | null;
  height: number | null;
  uploaderName: string | null;
  uploaderIsFallback: boolean;
  downloadUrl: string | null;
};
```

### 3. Consume in TypeScript
The React application imports the generated types directly, ensuring compile-time validation:
```typescript
import type { SafeAsset } from './types/generated/SafeAsset';

function AssetTile({ asset }: { asset: SafeAsset }) {
  return <div>{asset.originalFileName}</div>;
}
```

---

## 5. Security & Performance Hardening

### HTML Escaping
All dynamic values injected into SSR meta tags (`og:title`, `og:description`, `og:image`, `og:url`, and their Twitter equivalents) are HTML-entity-escaped via a dedicated `html_escape()` function in `main.rs`. This prevents stored XSS from malicious album names or descriptions.

### Cookie Security
The `Secure` flag on password session cookies (`immich_pwd_*`) is conditional: it is set only when the incoming `X-Forwarded-Proto` header is `https`. This allows password-protected shares to work on plain HTTP deployments (common in LAN/Docker setups without TLS termination).

### MIME Passthrough
`SafeAsset` carries `original_mime_type` from Immich's upstream API. The frontend uses this for `<video>` elements instead of hardcoding a MIME type, ensuring correct playback for formats like `video/quicktime` (`.mov`).

### Content-Disposition
Download responses include both an ASCII fallback `filename="..."` and a UTF-8 `filename*=UTF-8''...` parameter, ensuring filenames with non-ASCII characters display correctly across all browsers.

### OnceLock Caching
Environment variables and other process-wide config (e.g. `IPP_PUBLIC_BASE_URL`, `LEPTOS_SITE_ROOT`, `IMMICH_API_KEY_UPLOAD_USER`, the share-cache TTL, and the upload concurrency limit) and the shared `reqwest::Client` (with a 10-second connect timeout) are initialized once via `std::sync::OnceLock` and reused across requests.

### Share-Details Cache
`api/get_share_details.rs` keeps a small in-memory cache of fully-resolved share responses, keyed by share key (and password variant). Entries expire after `IPP_TTL_SHARE_CACHE_SECS` (default 45s) and are also proactively invalidated by the upload path whenever an album's contents change. A monotonic per-key generation counter is snapshotted before a rebuild and re-checked under the write lock, so an invalidation that races an in-flight rebuild correctly discards the stale result instead of caching it.

### Bulk Tag Cache
The `get_or_create_tag` function in `proxy.rs` populates the tag cache with all tags from a single `/tags` API call, rather than scanning the full list per lookup. This reduces upload latency for albums with many tags/uploaders.

### Upload Status Permission Cache
`/share/{key}/status` and `/share/{key}/status/stream` reuse a short-lived permission validation cache (`IPP_TTL_STATUS_LINK_CACHE_SECS`, default 60s) to avoid validating the share key on every single poll tick or SSE stream loop iteration. This prevents overloading the upstream Immich `/shared-links/me` endpoint.

### Upload Sessions Sweep
To prevent memory leaks from abandoned or failed upload sessions (e.g., if a client disconnects without sending the finish beacon), the in-memory `upload_sessions` registry maps session tokens to their pending asset sets and finished flag, running a lazy 600-second TTL cleanup sweep.

## 6. Development & Builds

### Running Locally
To run the proxy in development mode:
1. Start the React Vite dev server:
   ```bash
   cd frontend
   npm install
   npm run dev
   ```
2. In a separate shell, start the Rust Axum backend:
   ```bash
   IMMICH_URL=http://<immich-ip>:2283 IMMICH_API_KEY=<key> cargo run
   ```

### Production Bundling
Vite compiles the frontend assets directly to `target/site/`. Running the `deploy.sh` script or GitHub release workflow does the following:
```bash
# 1. Compile frontend static assets (Vite outDir is set to '../target/site')
(cd frontend && npm install && npm run build)

# 2. Build backend binary in release mode
cargo build --release
```
The resulting release binary (`target/release/immich-public-proxy-rs`) and static directory (`target/site/`) can be copied directly to your server.
