use axum::{body::Body, http::Request};
use futures_util::{SinkExt, StreamExt};
use my_turn::{broker::BrokerStats, create_app, room, room::RoomState, ws::WsResponse};
use std::{
    collections::HashSet,
    net::{SocketAddr, TcpListener},
};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tracing::log::warn;
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

async fn get_stats(addr: &str) -> BrokerStats {
    let client = hyper::Client::new();

    let response = client
        .request(
            Request::builder()
                .uri(format!("http://{}/stats", addr))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
    let body = std::str::from_utf8(&body).unwrap();
    let data: BrokerStats = serde_json::from_str(body).unwrap();
    data
}

// too lazy to write proper function signature
macro_rules! assert_frame_eq {
    ($stream:expr, $response:expr) => {{
        let item = $stream.next().await.unwrap().unwrap();
        if let Message::Text(text) = item {
            let text = std::str::from_utf8(text.as_bytes()).unwrap();
            let parsed_res = serde_json::from_str::<WsResponse>(text).unwrap();
            assert_eq!(parsed_res, $response);
        } else {
            warn!("item={}", item);
            panic!("Invalid response")
        }
    }};
}

// Spawn a server and talk to websocket endpoint
#[tokio::test]
async fn test_websocket() {
    let addr = start_server().await;
    let ws_url = format!("ws://{}/ws", addr);

    // start client 1
    let (mut stream_1, _) = connect_async(&ws_url).await.expect("Failed to connect");
    stream_1
        .send(Message::Text("Ok".to_string()))
        .await
        .unwrap();
    assert_frame_eq!(
        stream_1,
        WsResponse::Error {
            error: "Invalid message: error=Can't parse json, msg=Ok".to_string()
        }
    );
    assert_eq!(
        get_stats(&addr).await,
        BrokerStats {
            room_count: 0,
            session_count: 1
        }
    );

    // start client 2
    let (mut stream_2, _) = connect_async(&ws_url).await.expect("Failed to connect");
    assert_eq!(
        get_stats(&addr).await,
        BrokerStats {
            room_count: 0,
            session_count: 2
        }
    );

    // start client 3
    let (mut stream_3, _) = connect_async(&ws_url).await.expect("Failed to connect");
    assert_eq!(
        get_stats(&addr).await,
        BrokerStats {
            room_count: 0,
            session_count: 3
        }
    );

    // all clients try to join the same room
    for stream in [&mut stream_1, &mut stream_2, &mut stream_3] {
        stream
            .send(Message::Text(
                "{\"JoinRoom\":{\"room\":\"abc\"}}".to_string(),
            ))
            .await
            .unwrap();
        assert_frame_eq!(
            stream,
            WsResponse::Room(RoomState {
                name: "abc".to_string(),
                users: HashSet::new(),
                turns: Vec::new()
            })
        );
        assert_eq!(
            get_stats(&addr).await,
            BrokerStats {
                room_count: 1,
                session_count: 3
            }
        );
    }

    // client 1 and 2 register themselves with their names, client 2 only watch
    stream_1
        .send(Message::Text(
            "{\"Register\":{\"room\":\"abc\", \"username\":\"client 1\"}}".to_string(),
        ))
        .await
        .unwrap();
    let res = WsResponse::Room(RoomState {
        name: "abc".to_string(),
        users: HashSet::from(["client 1".to_string()]),
        turns: Vec::new(),
    });

    for stream in [&mut stream_1, &mut stream_2, &mut stream_3] {
        assert_frame_eq!(stream, res);
    }
    stream_2
        .send(Message::Text(
            "{\"Register\":{\"room\":\"abc\", \"username\":\"client 2\"}}".to_string(),
        ))
        .await
        .unwrap();
    let res = WsResponse::Room(RoomState {
        name: "abc".to_string(),
        users: HashSet::from(["client 1".to_string(), "client 2".to_string()]),
        turns: Vec::new(),
    });

    for stream in [&mut stream_1, &mut stream_2, &mut stream_3] {
        assert_frame_eq!(stream, res);
    }

    // client 1 send a chat message, everyone should receive it
    stream_1
        .send(Message::Text(
            "{\"SendChat\":{\"room\":\"abc\", \"message\":\"my message\"}}".to_string(),
        ))
        .await
        .unwrap();
    let res = WsResponse::Chat(room::ChatNotification {
        room: "abc".to_string(),
        username: "client 1".to_string(),
        message: "my message".to_string(),
    });
    for stream in [&mut stream_1, &mut stream_2, &mut stream_3] {
        assert_frame_eq!(stream, res);
    }

    // client 1 and client 2 try to take their turns
    stream_1
        .send(Message::Text(
            "{\"TakeTurn\":{\"room\":\"abc\"}}".to_string(),
        ))
        .await
        .unwrap();
    let res = WsResponse::Room(RoomState {
        name: "abc".to_string(),
        users: HashSet::from(["client 1".to_string(), "client 2".to_string()]),
        turns: Vec::from(["client 1".to_string()]),
    });
    for stream in [&mut stream_1, &mut stream_2, &mut stream_3] {
        assert_frame_eq!(stream, res);
    }
    stream_2
        .send(Message::Text(
            "{\"TakeTurn\":{\"room\":\"abc\"}}".to_string(),
        ))
        .await
        .unwrap();
    let res = WsResponse::Room(RoomState {
        name: "abc".to_string(),
        users: HashSet::from(["client 1".to_string(), "client 2".to_string()]),
        turns: Vec::from(["client 1".to_string(), "client 2".to_string()]),
    });
    for stream in [&mut stream_1, &mut stream_2, &mut stream_3] {
        assert_frame_eq!(stream, res);
    }

    // Take turn another time doesn't count
    stream_2
        .send(Message::Text(
            "{\"TakeTurn\":{\"room\":\"abc\"}}".to_string(),
        ))
        .await
        .unwrap();
    let res = WsResponse::Room(RoomState {
        name: "abc".to_string(),
        users: HashSet::from(["client 1".to_string(), "client 2".to_string()]),
        turns: Vec::from(["client 1".to_string(), "client 2".to_string()]),
    });
    for stream in [&mut stream_1, &mut stream_2, &mut stream_3] {
        assert_frame_eq!(stream, res);
    }

    // Reset turn for the next round
    stream_2
        .send(Message::Text(
            "{\"ResetTurn\":{\"room\":\"abc\"}}".to_string(),
        ))
        .await
        .unwrap();
    let res = WsResponse::Room(RoomState {
        name: "abc".to_string(),
        users: HashSet::from(["client 1".to_string(), "client 2".to_string()]),
        turns: Vec::new(),
    });
    for stream in [&mut stream_1, &mut stream_2, &mut stream_3] {
        assert_frame_eq!(stream, res);
    }

    // stop client 1
    drop(stream_1);
    assert_eq!(
        get_stats(&addr).await,
        BrokerStats {
            room_count: 1,
            session_count: 2
        }
    );

    // stop client 2
    drop(stream_2);
    assert_eq!(
        get_stats(&addr).await,
        BrokerStats {
            room_count: 1,
            session_count: 1
        }
    );

    // stop client 2
    drop(stream_3);
    assert_eq!(
        get_stats(&addr).await,
        BrokerStats {
            room_count: 0,
            session_count: 0
        }
    );
}
