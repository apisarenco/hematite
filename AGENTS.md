# Hematite Architecture & Agent Guide

Hematite is a high-performance, single-binary Markdown CMS engine written in Rust (edition 2021). It ingests directories of Markdown files (with native Obsidian support) and serves them with sub-millisecond server-side rendering (SSR), dynamic single-page application (SPA) region hydration, live filesystem synchronization, and stateless token-versioned authentication/authorization.

---

## 1. High-Level Architecture & End-to-End Data Flow

The following sequence illustrates how content moves from a raw Markdown file on disk to a rendered webpage in the browser:

```mermaid
flowchart TD
    MD[("Markdown Files\n(./content)")] -->|File Watcher / Initial Crawl| Crawler["Crawler Engine\n(walkdir + xxh3_128)"]
    CFG[("hematite.yaml")] -->|Load config| Config["HematiteConfig"]
    
    subgraph Ingestion & Compilation
        Crawler -->|Parse AST + YAML Frontmatter| MDParser["MarkdownEngine\n(pulldown-cmark -> DocAst)"]
        CompilePlugins["Compile-Time Plugins\n(build.rs -> register_all)\n(Wikilinks, Admonitions)"] -.-> MDParser
        MDParser -->|CBOR Serialization (ciborium)| PageRecord["PageRecord\n(ast_blob + build_hash)"]
        PageRecord --> MPSC["Tokio MPSC Channel\n(DbWriteCommand)"]
        MPSC -->|Single-Writer Actor| Writer["SQLite Writer Actor\n(WAL mode)"]
        Writer -->|Upsert Pages, FTS5 & Links| DB[("SQLite Database\n(pages, directories, page_links, fts5, system_meta)")]
        
        Writer -->|Post-crawl Extensibility| Hooks["Extensibility Hooks\n(Lua / Rhai with entity.ast)"]
        Config -.-> Hooks
        Hooks -->|Update Titles, Frontmatter, AST| Writer
    end

    subgraph Background Reconciliation & Invalidation
        Watcher["File Watcher (themes/ + scripts/)"] -->|Change Detected| CompHash["Compute Hash (xxh3_128)"]
        CompHash -->|Set current_build_hash| DB
        CompHash -->|Pulse Notify| Reconciler["Reconciliation Worker (tokio background)"]
        Reconciler -->|Fetch Outdated Pages| DB
        Reconciler -->|Element Renderer (Minijinja / Hardcoded)| Reconciler
        Reconciler -->|OCC Update (WHERE content_hash = ?)| Writer
    end

    subgraph Serving & Request Pipeline
        Client["Browser Client\n(SPA / SSR)"] -->|HTTP GET /path| Axum["Axum Web Router"]
        Axum -->|Session / Bearer JWT| Auth["ACL Middleware\n(Token version + path glob matching)"]
        Auth -->|Read-only pool query| DB
        
        DB --> FreshCheck{"build_hash == current?"}
        FreshCheck -->|Yes| FastServe["Serve cached HTML (< 1ms)"]
        FreshCheck -->|No (Cache Miss)| JIT["JIT Render via ElementRenderer\n(Minijinja element templates or fast emitter)"]
        JIT --> AsyncCache["Background update SQLite cache"]
        AsyncCache -.-> Writer
        JIT --> RenderTemplates
        FastServe --> RenderTemplates

        RenderTemplates["Minijinja Template Engine\n(page.html layout + Theme Overrides)"]
        RenderTemplates -->|Full HTML| SSR["SSR Full Refresh"]
        RenderTemplates -->|JSON Regions: content, sidebar, toc| API["SPA JSON API\n(/_api/v1/page)"]
        
        SSR --> Client
        API --> Client
    end
```

---

## 2. Core Features Breakdown

### 2.1 Configuration System (`hematite.yaml`)
- **Discovery**: Automatically discovers configuration in explicit CLI flags (`--config`), `./hematite.yaml`, `./hematite.yml`, or `<content_dir>/../hematite.yaml`.
- **Top-Level Settings**:
  - `name`: Site brand/title.
  - `base_url`: Base canonical URL for canonical tags, sitemaps, and feeds.
  - `theme`: Active theme directory under `./themes/<theme_name>`.
  - `anonymous_user_display`: Display name for unauthenticated users (defaults to `"Anonymous"`).
- **Free-Form Attributes**: Arbitrary extra YAML keys (e.g. `campaign`, `edition`, `author`) are deserialized and exposed directly to Minijinja templates (`config.<key>`) as well as Lua and Rhai extensibility hooks (`config.<key>`).

### 2.2 Ingestion, Crawling & SQLite Single-Writer Architecture
- **Diffing with `xxh3_128`**: During initial boot and continuous crawler syncs, file bytes are hashed using 128-bit xxHash (`xxh3_128`). Files whose content hash hasn't changed skip AST re-parsing.
- **Obsolete Page Pruning**: Files deleted while the server was offline are detected, cleaned from SQLite, and removed from the FTS5 search index.
- **Single-Writer Actor (`DbActor`)**: SQLite runs with `journal_mode = WAL` and `synchronous = NORMAL`. All write operations (`UpsertPage`, `RemoveFile`, `UpdatePageFromHook`, `RebuildDirectoryTree`) are dispatched via an asynchronous `tokio::sync::mpsc` queue to a dedicated actor thread. This completely eliminates SQLite database lock contention (`SQLITE_BUSY`).
- **Concurrent Readers**: Web requests read concurrently through an `r2d2` connection pool with zero lock delays.
- **Virtual Directory Tree**: Directories do not require `_index.md` or `index.md`. If absent, Hematite generates virtual directory listings automatically from parent-child slug paths.
- **File Watcher (`notify`)**: Debounced at 50ms to detect create, write, and remove file operations in real time without restarting the daemon.

### 2.3 Compile-Time Plugin System
Plugins extend Markdown parsing at compile time with zero runtime dynamic dispatch penalty:
- **Build Discovery (`build.rs`)**:
  - Automatically scans `src/plugins/*.rs`.
  - Generates a static registration manifest (`plugins_manifest.rs`) in `OUT_DIR`.
- **Trait Definition (`MarkdownPlugin`)**:
  - `trigger_byte()`: Hot-loop filter character (e.g., `[` for wikilinks, `>` for admonitions/callouts) to bypass unnecessary transforms on plain text.
  - `transform()` and `transform_with_context()`: Rewrites `pulldown_cmark::Event` streams into target HTML/DOM events.
- **Built-in Plugins**:
  - **Wikilinks & Embeds**: Resolves Obsidian links `[[Target]]` or `[[Target|Alias]]` and image embeds `![[image.png]]` against the SQLite link graph.
  - **Admonitions**: Transforms Obsidian callout blocks (`> [!NOTE]`, `> [!WARNING]`, etc.) into structured `<div class="admonition ...">` or collapsible `<details class="admonition ...">` tags.

### 2.4 Extensibility Hooks (Lua & Rhai)
Hematite supports post-parse scripting hooks in `scripts/hooks/file/` and `scripts/hooks/directory/`:
- **Execution Order**: Sorted alphabetically by filename (e.g. `01_clean_title.lua`, `02_verifier.rhai`).
- **Scope & Context**:
  - Scripts receive `entity`, `context` (contains `all_pages`, `all_directories`, and `config`), and a global `config` shortcut.
  - Modified entity attributes (such as `title`, `url_path`, `frontmatter_json`, `parsed_html`) are saved back to SQLite.

### 2.5 Dual-Mode Rendering: Full SSR & SPA Region Hydration
Hematite serves both traditional search engine crawlers and modern fast-navigation single-page web apps:
- **Full Refresh (SSR)**: Standard HTTP GET requests render `page.html` or `directory_index.html` with breadcrumbs, sidebar navigation tree, and article body.
- **Client-Side SPA (`static/js/app.js`)**:
  - Intercepts internal `<a>` link clicks and `popstate` events.
  - Fetches partial content via `/_api/v1/page?path=<url_path>`.
  - Swaps out inner HTML for targeted regions (`#content-area`, `#sidebar-nav`, `#toc`) while smoothly scrolling hash anchors and updating active nav states without full browser reloads.
  - Gracefully falls back to full HTTP page navigation if an endpoint fails or points outside the document graph.

### 2.6 Theming System (`themes/`)
- **Template Inheritance & Overrides**: The Minijinja engine first loads embedded default templates, then overlays any template files placed under `./themes/<theme_name>/templates/`.
- **Theme Assets**: Theme-specific static assets are served under `/_static/theme/*` from `./themes/<theme_name>/static/`.

### 2.7 Search Engine
- **Full-Text Search (FTS5)**: Integrated SQLite FTS5 table indexing titles, parsed text, and taxonomy metadata with Porter stemmer tokenization.
- **Search API & UI**:
  - JSON endpoint: `/_api/v1/search?q=<query>`
  - HTML search interface: `/_search?q=<query>`

### 2.8 Authentication, ACL & User Administration
- **Stateless JWT**: Authentication uses Bearer tokens or `auth_token` cookies signed with HMAC-SHA256 (`jwt_secret`).
- **Token Versioning**: Every user record in SQLite maintains a `token_version`. Whenever an admin modifies or revokes permissions, `token_version` increments, instantly invalidating outstanding JWTs without requiring server-side session stores.
- **Access Control Lists (ACLs)**:
  - Rules evaluate `verb` (`ALLOW` or `DENY`) against path glob patterns (e.g. `/**`, `/admin/**`, `/secret/*`).
  - Prioritized matching with support for wildcard user `*` (anonymous/public) and named user roles.
  - Enforced globally via Axum middleware layer `verify_acl`.
- **Admin Panel**: Accessible at `/_admin/` for user creation, user deletion, token version bumping, and ACL rule management.

---

## 3. Directory Layout

```
.
├── Cargo.toml               # Project dependencies and workspace config
├── build.rs                 # Compile-time plugin discovery and code generation
├── hematite.yaml            # Runtime site configuration (theme, name, metadata)
├── solution-design.md       # Technical specification and architecture design doc
├── scripts/
│   ├── README.md            # Extensibility hooks documentation and examples
│   └── hooks/
│       ├── file/            # Per-page hooks (*.lua, *.rhai)
│       └── directory/       # Per-directory hooks (*.lua, *.rhai)
├── src/
│   ├── main.rs              # CLI initialization, runtime orchestration, shutdown signals
│   ├── lib.rs               # Library export for module reusability and integration tests
│   ├── auth/                # JWT manager, Argon2 password hashing, and ACL verification
│   ├── config/              # YAML configuration loader with fallback hierarchy
│   ├── crawler/             # Filesystem scanner, xxh3 hashing, slug/url derivation
│   ├── db/                  # SQLite migrations, single-writer actor, read connection pool
│   ├── markdown/            # pulldown-cmark pipeline and AST rewriting
│   ├── plugin.rs            # MarkdownPlugin trait & TransformContext definition
│   ├── plugins/             # Compile-time plugins (admonitions, wikilinks)
│   ├── scripts/             # Lua (mlua) and Rhai engine bindings for hooks
│   ├── server/              # Axum HTTP server, routing, SSR/SPA endpoints, and admin UI
│   ├── template/            # Minijinja template renderer and theme override loader
│   └── watcher/             # File watcher service using notify crate
├── templates/               # Default embedded HTML templates & components
├── themes/                  # Theme bundles (e.g. themes/bemm/)
└── static/                  # Built-in CSS, SPA navigation client JavaScript, and vendor assets
```

---

## 4. Key Developer & Agent Workflows

### 4.1 Running Tests
```bash
cargo test
```

### 4.2 Starting the Daemon
```bash
cargo run -- --content ./content --themes ./themes --config ./hematite.yaml
```

### 4.3 Adding a Compile-Time Markdown Plugin
1. Create a new file `src/plugins/<my_plugin>.rs`.
2. Implement the `MarkdownPlugin` trait for `pub struct PluginInstance;`.
3. Optionally specify `trigger_byte(&self) -> Option<u8>` for fast scanning.
4. Run `cargo build` — `build.rs` automatically discovers the new file and registers it in `plugins_manifest.rs`.

### 4.4 Adding Extensibility Hooks
- Place `.lua` or `.rhai` files in `scripts/hooks/file/` or `scripts/hooks/directory/`.
- Access and mutate `entity`, use `context`, and read `config` properties.
- Scripts execute automatically during filesystem indexing.
