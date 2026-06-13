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
├── bindings/                   # (Cleaned up) temporary bindings folder
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
If the share link allows uploading, users can upload photos/videos. The client prompts for an uploader name (saved in `localStorage`), attaches it as a custom header, and sends files to `/share/:key/upload`. The backend proxies this directly to Immich and tags the uploaded files.

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
    Axum-->>User: Return 200 OK / Success
```

### C. ZIP Downloads
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
    pub width: Option<i32>,
    pub height: Option<i32>,
}
```

### 2. Export on Test Execution
Running `cargo test` executes the `ts-rs` runner, writing corresponding `.ts` interface declarations:
```typescript
// Automatically generated by ts-rs. Do not edit.
export interface SafeAsset {
  id: string;
  originalFileName: string | null;
  width: number | null;
  height: number | null;
}
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

## 5. Development & Builds

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
