
use std::{net::SocketAddr};

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use my_turn::create_app;

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "example_websockets=debug,tower_http=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // run it with hyper
    let addr = SocketAddr::from(([127, 0, 0, 1], 8080));
    tracing::debug!("listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(create_app().into_make_service())
        .await
        .unwrap();
}
