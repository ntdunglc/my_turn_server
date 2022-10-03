use crate::server;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        TypedHeader,
    },
    headers,
    response::IntoResponse,
};
use futures_util::{
    stream::{SplitSink, SplitStream},
    FutureExt, SinkExt, StreamExt, TryFutureExt,
};
use serde::{Deserialize, Serialize};
use std::{fmt::Debug, time::Duration};
use tokio::sync::mpsc::{self, Sender, UnboundedSender};
use tokio::sync::oneshot;
use tokio::time::Instant;
use tracing::log::{debug, warn};
pub use usize as SessionId;

/// How often heartbeat pings are sent
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// How long before lack of client response causes a timeout
const CLIENT_TIMEOUT: Duration = Duration::from_secs(16);

#[derive(Debug)]
pub struct ClientSession {
    pub id: SessionId,
    // pub room_manager: Addr<room_manager::RoomManager>,
    /// Client must send ping at least once per 10 seconds (CLIENT_TIMEOUT),
    /// otherwise we drop connection.
    pub hearbeat: Instant,
}

#[derive(Serialize, Deserialize)]
pub enum MailboxMessage {
    // SocketMessage { message: String},
    CreateRoom { room: String },
    Vote { room: String },
    Chat { room: String, message: String },
}

// #[derive(Serialize, Deserialize)]
// pub enum Response {
//     ClientState { id: SessionId },
//     // Room { room: Room },
//     Chat { message: String },
//     Error { error: String },
// }

async fn send_heartbeat(tx: UnboundedSender<Message>) -> Result<(), anyhow::Error> {
    let last_heartbeat = Instant::now();
    loop {
        if Instant::now().duration_since(last_heartbeat) > CLIENT_TIMEOUT {
            // heartbeat timed out
            warn!("Websocket Client heartbeat failed, disconnecting!");
            // act.room_manager
            //     .do_send(room_manager::Disconnect { id: act.id });
            tx.send(Message::Close(None))?;
            break;
        } else {
            tx.send(Message::Ping(b"".to_vec()))?;
        }
        tokio::time::sleep(HEARTBEAT_INTERVAL).await;
    }
    Ok(())
}

async fn handle_ws_messages(
    sender: UnboundedSender<Message>,
    receiver: &mut SplitStream<WebSocket>,
) -> Result<(), anyhow::Error> {
    while let Some(msg) = receiver.next().await {
        if let Ok(msg) = msg {
            match msg {
                Message::Text(text) => {
                    debug!("client sent str: {:?}", text);
                    if let Err(_) = sender.send(Message::Text(text)) {
                        break;
                    };
                }
                Message::Binary(_) => {
                    debug!("client sent binary data");
                    break;
                }
                Message::Ping(_) => {
                    debug!("socket ping");
                }
                Message::Pong(_) => {
                    debug!("socket pong");
                }
                Message::Close(_) => {
                    debug!("client disconnected");
                    break;
                }
            }
        } else {
            debug!("client disconnected");
            break;
        }
    }
    Ok(())
}

async fn handle_mailbox_message(server_addr: Sender<server::MailboxMessage>) {}

// utility to convert sink to sender, as sink can't be cloned
fn sink_to_sender<S, Item>(mut sink: SplitSink<S, Item>) -> UnboundedSender<Item>
where
    SplitSink<S, Item>: SinkExt<Item> + Send + 'static,
    <SplitSink<S, Item> as futures_util::Sink<Item>>::Error: Debug,
    Item: Send + Sync,
{
    let (tx, mut rx) = mpsc::unbounded_channel::<Item>();

    tokio::task::spawn(async move {
        while let Some(msg) = rx.recv().await {
            // In any websocket error, break loop.
            if let Err(e) = sink.send(msg).await {
                warn!("websocket send error: {:?}", e);
                break;
            }
        }
    });
    tx
}

async fn connect(
    server_addr: mpsc::Sender<server::MailboxMessage>,
) -> Result<SessionId, anyhow::Error> {
    let (send, recv) = oneshot::channel();
    let msg = server::MailboxMessage::Connect { respond_to: send };

    server_addr.send(msg).await?;
    Ok(recv.await?)
}

async fn disconnect(
    session_id: SessionId,
    server_addr: mpsc::Sender<server::MailboxMessage>,
) -> Result<(), anyhow::Error> {
    let msg = server::MailboxMessage::Disconnect {
        session_id: session_id,
    };
    server_addr.send(msg).await?;
    Ok(())
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    server_addr: Sender<server::MailboxMessage>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| {
        handle_socket(socket, server_addr).map(|res| {
            if let Err(err) = res {
                warn!("Error handling socket {}", err);
            }
        })
    })
}

async fn handle_socket(
    socket: WebSocket,
    server_addr: Sender<server::MailboxMessage>,
) -> Result<(), anyhow::Error> {
    let (socket_sender, mut socket_receiver) = socket.split();
    let sender = sink_to_sender(socket_sender);

    // This task will receive broadcast messages and send text message to our client.
    let sender_hb = sender.clone();
    let mut hearbeat_task = tokio::spawn(async move { send_heartbeat(sender_hb).await });

    let session_id = connect(server_addr.clone()).await?;

    let (mailbox_tx, mut mailbox_rx) = mpsc::unbounded_channel::<MailboxMessage>();
    let mut mailbox_recv_task = tokio::spawn(async move {
        while let Some(msg) = mailbox_rx.recv().await {
            // do something here
        }
    });

    // This task will receive messages from client and send them to broadcast subscribers.
    let mut socket_recv_task =
        tokio::spawn(async move { handle_ws_messages(sender, &mut socket_receiver).await });
    debug!("spawned recv_task and send_task");

    // If any one of the tasks exit, abort the other.
    tokio::select! {
        _ = (&mut mailbox_recv_task) => socket_recv_task.abort(),
        _ = (&mut socket_recv_task) => mailbox_recv_task.abort(),
    };

    hearbeat_task.abort();

    disconnect(session_id, server_addr).await?;
    debug!("done, closing");
    Ok(())
}
