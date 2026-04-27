//! Serve SPA static assets embedded into the binary via `rust-embed`.
//!
//! Only compiled when the `embedded-frontend` Cargo feature is enabled.
//! In that mode, `web/dist/` is required at build time and is bundled into
//! the binary; the OSS build skips this module entirely.
//!
//! Strategy:
//! - Try exact path match first (e.g. `/assets/main-abc123.js`).
//! - Otherwise fall back to `index.html` — lets a client-side router handle
//!   deep URLs (`/doc/std::io::BufReader`) without 404.

use axum::extract::Request;
use axum::http::{header, Uri};
use axum::response::{IntoResponse, Response};

#[derive(rust_embed::RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../web/dist"]
pub(crate) struct WebAssets;

#[allow(clippy::unused_async)]
pub(crate) async fn serve(req: Request) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    if let Some(response) = serve_path(path) {
        return response;
    }
    if let Some(response) = serve_path("index.html") {
        return response;
    }
    // Should be unreachable when the `embedded-frontend` feature is enabled
    // (the build pipeline must have produced a valid `web/dist/`); kept as
    // defensive fallback for malformed Pro builds.
    missing_index(req.uri())
}

fn serve_path(path: &str) -> Option<Response> {
    let asset = WebAssets::get(path)?;
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Some(
        (
            [(header::CONTENT_TYPE, mime.as_ref())],
            asset.data.into_owned(),
        )
            .into_response(),
    )
}

fn missing_index(uri: &Uri) -> Response {
    let body = format!(
        "<!doctype html>\
         <meta charset=utf-8>\
         <title>Standardoc Pro — broken build</title>\
         <body style=\"font-family:system-ui;padding:2rem;max-width:40rem\">\
         <h1>Frontend assets missing</h1>\
         <p>Tried to serve <code>{uri}</code> but no <code>index.html</code> was found in the embedded bundle. \
         The Pro build pipeline produced a malformed <code>web/dist/</code>. Reach out to support.</p>\
         </body>"
    );
    (
        axum::http::StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}
