use axum::{
    extract::Extension,
    http::StatusCode,
    routing::{get, get_service},
    Router,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc::{self, Sender};
use tower_http::{
    services::ServeDir,
    trace::{DefaultMakeSpan, TraceLayer},
};

pub mod error;
pub mod server;
pub mod ws;

pub fn create_app() -> Router {
    let server_addr = server::Server::start();

    let assets_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");
    Router::new()
        .fallback(
            get_service(ServeDir::new(assets_dir).append_index_html_on_directories(true))
                .handle_error(|error: std::io::Error| async move {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Unhandled internal error: {}", error),
                    )
                }),
        )
        // routes are matched from bottom to top, so we have to put `nest` at the
        // top since it matches all routes
        .route("/ws", get(move |ws| ws::ws_handler(ws, server_addr)))
        // logging so we can see whats going on
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::default().include_headers(true)),
        )
}
