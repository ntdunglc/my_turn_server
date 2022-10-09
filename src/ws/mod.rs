use crate::{actor::Actor, broker::BrokerAddr, user_session};
use axum::{
    extract::ws::{WebSocket, WebSocketUpgrade},
    response::IntoResponse,
};
use futures_util::{FutureExt, StreamExt};

use tracing::log::warn;
pub use usize as SessionId;

pub mod reader;
pub mod writer;

pub async fn ws_handler(ws: WebSocketUpgrade, broker_addr: BrokerAddr) -> impl IntoResponse {
    ws.on_upgrade(move |socket| {
        handle_socket(socket, broker_addr).map(|res| {
            if let Err(err) = res {
                warn!("Error handling socket {}", err);
            }
        })
    })
}

async fn handle_socket(socket: WebSocket, broker_addr: BrokerAddr) -> Result<(), anyhow::Error> {
    let (socket_sender, socket_receiver) = socket.split();
    let ws_writer_addr = writer::WsWriter {
        sink: socket_sender,
    }
    .spawn();
    let user_session_addr =
        user_session::UserSession::new(broker_addr, ws_writer_addr.clone()).spawn();

    let _ = reader::WsReader {
        receiver: socket_receiver,
        ws_writer_addr: ws_writer_addr,
        user_session_addr,
    }
    .spawn();

    Ok(())
}
