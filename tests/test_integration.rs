use futures_util::{SinkExt, StreamExt};
use my_turn::create_app;
use serde_json::{json, Value};
use std::net::{SocketAddr, TcpListener};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

async fn start_server() -> String {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "my_turn=debug,tower_http=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();
    let listener = TcpListener::bind("0.0.0.0:0".parse::<SocketAddr>().unwrap()).unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::Server::from_tcp(listener)
            .unwrap()
            .serve(create_app().into_make_service())
            .await
            .unwrap();
    });
    format!("127.0.0.1:{}", addr.port())
}

// Spawn a server and talk to websocket endpoint
#[tokio::test]
async fn test_websocket() {
    let addr = start_server().await;
    let url = format!("ws://{}/ws", addr);
    let (mut stream, _) = connect_async(url).await.expect("Failed to connect");
    stream.send(Message::Text("Ok".to_string())).await.unwrap();
    let item = stream.next().await.unwrap().unwrap();
    assert_eq!(item, Message::Ping(b"".to_vec()));
    let item = stream.next().await.unwrap().unwrap();
    assert_eq!(item, Message::Text("Ok".to_string()));
}
