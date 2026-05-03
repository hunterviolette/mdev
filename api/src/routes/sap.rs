use axum::{routing::post, Json, Router};

use crate::{
    app_state::AppState,
    engine::capabilities::sap::runtime::{
        fetch_object_manifest,
        search_package_objects,
        ObjectRequest,
        ObjectResponse,
        SearchRequest,
        SearchResponse,
    },
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/sap/search", post(search))
        .route("/api/sap/object", post(object))
}

async fn search(
    Json(req): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, (axum::http::StatusCode, String)> {
    search_package_objects(req).map(Json).map_err(internal)
}

async fn object(
    Json(req): Json<ObjectRequest>,
) -> Result<Json<ObjectResponse>, (axum::http::StatusCode, String)> {
    fetch_object_manifest(req).map(Json).map_err(internal)
}

fn internal<E: std::fmt::Display>(err: E) -> (axum::http::StatusCode, String) {
    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
}
