//! REST handlers. Each endpoint is an async function that takes
//! `State<Arc<dyn WebState>>` and returns JSON through axum extractors.
//!
//! Handlers contain no business logic: they route to `WebState` trait methods.
//! All business logic (markdown rendering, search, ranking) lives in the
//! concrete `standardoc-server` implementation, not here. This keeps the web
//! crate testable without booting a real workspace.

use crate::state::WebState;
use crate::types::{
    DeletePageError, IndexResponse, PagesResponse, ReorderPageError, ReorderPageRequest,
    ResolvedSourceConfig, SavePageError, SavePageRequest, SearchResponse,
};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[allow(clippy::unused_async)]
pub(crate) async fn health(State(state): State<Arc<dyn WebState>>) -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "revision": state.revision(),
    }))
}

#[allow(clippy::unused_async)]
pub(crate) async fn index(State(state): State<Arc<dyn WebState>>) -> Json<IndexResponse> {
    Json(IndexResponse {
        revision: state.revision(),
        workspace_root: state.workspace_root().display().to_string(),
        blocks: state.list_blocks(),
    })
}

#[allow(clippy::unused_async)]
pub(crate) async fn doc(
    State(state): State<Arc<dyn WebState>>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    state.get_doc(&key).map_or_else(
        || {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "doc not found", "key": key })),
            )
                .into_response()
        },
        |doc| Json(doc).into_response(),
    )
}

#[derive(Debug, Deserialize)]
pub(crate) struct SearchQuery {
    pub q: String,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
}

const fn default_search_limit() -> usize {
    20
}

#[allow(clippy::unused_async)]
pub(crate) async fn search(
    State(state): State<Arc<dyn WebState>>,
    Query(params): Query<SearchQuery>,
) -> Json<SearchResponse> {
    let matches = state.search(&params.q, params.limit);
    Json(SearchResponse {
        revision: state.revision(),
        query: params.q,
        matches,
    })
}

#[allow(clippy::unused_async)]
pub(crate) async fn config(State(state): State<Arc<dyn WebState>>) -> Json<ResolvedSourceConfig> {
    // Daemon mode → is_static_export = false. The static export path bakes
    // its own ResolvedSourceConfig into static-data.json with `is_static_export = true`.
    Json(state.source_config(false))
}

#[allow(clippy::unused_async)]
pub(crate) async fn dsl_reference(State(state): State<Arc<dyn WebState>>) -> impl IntoResponse {
    let body = state.dsl_reference_markdown().to_owned();
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/markdown; charset=utf-8",
        )],
        body,
    )
}

#[allow(clippy::unused_async)]
pub(crate) async fn pages(State(state): State<Arc<dyn WebState>>) -> Json<PagesResponse> {
    Json(PagesResponse {
        revision: state.revision(),
        pages: state.list_pages(),
    })
}

#[allow(clippy::unused_async)]
pub(crate) async fn page(
    State(state): State<Arc<dyn WebState>>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    state.get_page(&slug).map_or_else(
        || {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "page not found", "slug": slug })),
            )
                .into_response()
        },
        |page| Json(page).into_response(),
    )
}

/// Special case: home page = empty `slug ""`. Axum does not like empty path
/// captures, so we expose a dedicated `/api/page` route (without parameter)
/// that returns the root page.
#[allow(clippy::unused_async)]
pub(crate) async fn page_home(State(state): State<Arc<dyn WebState>>) -> impl IntoResponse {
    state.get_page("").map_or_else(
        || {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "no home page" })),
            )
                .into_response()
        },
        |page| Json(page).into_response(),
    )
}

#[allow(clippy::unused_async)]
pub(crate) async fn put_page(
    State(state): State<Arc<dyn WebState>>,
    Path(slug): Path<String>,
    Json(body): Json<SavePageRequest>,
) -> impl IntoResponse {
    save_page_response(&state, &slug, &body.source)
}

#[allow(clippy::unused_async)]
pub(crate) async fn put_page_home(
    State(state): State<Arc<dyn WebState>>,
    Json(body): Json<SavePageRequest>,
) -> impl IntoResponse {
    save_page_response(&state, "", &body.source)
}

fn save_page_response(
    state: &Arc<dyn WebState>,
    slug: &str,
    source: &str,
) -> axum::response::Response {
    match state.save_page(slug, source) {
        Ok(page) => Json(page).into_response(),
        Err(SavePageError::InvalidSlug) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_slug", "slug": slug })),
        )
            .into_response(),
        Err(SavePageError::IoError) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "io_error" })),
        )
            .into_response(),
    }
}

#[allow(clippy::unused_async)]
pub(crate) async fn delete_page(
    State(state): State<Arc<dyn WebState>>,
    Path(slug): Path<String>,
) -> impl IntoResponse {
    match state.delete_page(&slug) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(DeletePageError::InvalidSlug) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_slug", "slug": slug })),
        )
            .into_response(),
        Err(DeletePageError::NotFound) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "not_found", "slug": slug })),
        )
            .into_response(),
        Err(DeletePageError::NotOnDisk) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "not_on_disk", "slug": slug })),
        )
            .into_response(),
        Err(DeletePageError::IoError) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "io_error" })),
        )
            .into_response(),
    }
}

#[allow(clippy::unused_async)]
pub(crate) async fn patch_page(
    State(state): State<Arc<dyn WebState>>,
    Path(slug): Path<String>,
    Json(body): Json<ReorderPageRequest>,
) -> impl IntoResponse {
    match state.reorder_page(&slug, body.order) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(ReorderPageError::InvalidSlug) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_slug" })),
        )
            .into_response(),
        Err(ReorderPageError::NotOnDisk) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": "not_on_disk" })),
        )
            .into_response(),
        Err(ReorderPageError::IoError) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "io_error" })),
        )
            .into_response(),
    }
}
