//! HTTP transport for Standardoc.
//!
//! Serves REST + SSE under `/api/*` on a single port. Frontend serving is
//! gated behind the `embedded-frontend` Cargo feature — disabled in the
//! open-source build (any non-`/api/*` route returns a placeholder pointing
//! at the REST API + Standardoc Pro), enabled in the Pro build (where it
//! embeds the React/Vite SPA from `standardoc/web/dist/` via `rust-embed`).
//!
//! This crate does not own the index: it receives an `Arc<dyn WebState>`
//! built by the caller (in practice `standardoc-server`) at boot and kept
//! alive for daemon lifetime.

mod api;
#[cfg(feature = "embedded-frontend")]
mod embed;
pub mod export;
pub mod highlight;
mod sse;
pub mod state;
pub mod types;

pub use state::WebState;

use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

async fn serve_syntax_css() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        highlight::syntax_css(),
    )
}

/// Build the full axum router (REST + SSE + frontend if `embedded-frontend`).
///
/// Caller mounts this with `axum::serve(listener, router(state))`.
/// No global state: everything goes through `Arc<dyn WebState>` extracted
/// by handlers with `axum::extract::State`.
pub fn router(state: Arc<dyn WebState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let router = Router::new()
        .route("/api/health", get(api::health))
        .route("/api/index", get(api::index))
        .route("/api/doc/{key}", get(api::doc))
        .route("/api/search", get(api::search))
        .route("/api/dsl-reference", get(api::dsl_reference))
        .route("/api/config", get(api::config))
        .route("/api/pages", get(api::pages))
        .route("/api/page", get(api::page_home).put(api::put_page_home))
        .route(
            "/api/page/{*slug}",
            get(api::page)
                .put(api::put_page)
                .patch(api::patch_page)
                .delete(api::delete_page),
        )
        .route("/api/events", get(sse::events))
        .route("/api/syntax.css", get(serve_syntax_css));

    #[cfg(feature = "embedded-frontend")]
    let router = router.fallback(embed::serve);
    #[cfg(not(feature = "embedded-frontend"))]
    let router = router.fallback(no_frontend_placeholder);

    router.with_state(state).layer(cors)
}

/// Placeholder served when the binary is built without the `embedded-frontend`
/// feature. Points users to the REST API and to Standardoc Pro for the
/// official UI.
#[cfg(not(feature = "embedded-frontend"))]
async fn no_frontend_placeholder(req: axum::extract::Request) -> impl IntoResponse {
    let uri = req.uri().clone();
    let body = format!(
        "<!doctype html>\
         <meta charset=utf-8>\
         <title>Standardoc — no frontend bundled</title>\
         <body style=\"font-family:system-ui;padding:2rem;max-width:42rem;line-height:1.5\">\
         <h1>No web frontend bundled in this binary</h1>\
         <p>Tried to serve <code>{uri}</code> but this binary was built without \
         the <code>embedded-frontend</code> Cargo feature — the standard \
         configuration of the open-source <code>standardoc-server</code>.</p>\
         <h2>What you can do</h2>\
         <ul>\
           <li>Use the REST API at <code>/api/*</code> directly (index, docs, search, pages, SSE events)</li>\
           <li>Build your own frontend against the documented endpoints</li>\
           <li>Get the official UI: <a href=\"https://standardoc.miralabs.tech\"><strong>Standardoc Pro</strong></a> (lifetime license)</li>\
         </ul>\
         <p style=\"color:#666;font-size:.9em\">The MCP and LSP transports are unaffected — \
         this placeholder only concerns the HTTP web view.</p>\
         </body>"
    );
    (
        axum::http::StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
}
