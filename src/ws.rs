use crate::{
    broker::{self, BrokerAddr},
    room::{self, RoomAddr},
    utils::spawn_and_log_error,
};
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::IntoResponse,
};
use futures_util::{
    stream::{SplitSink, SplitStream},
    FutureExt, SinkExt, StreamExt,
};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt::Debug, time::Duration};
use tokio::sync::mpsc::{self, Receiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::time::{timeout, Instant};
use tracing::log::{debug, warn};
pub use usize as SessionId;

const CHANNEL_SIZE: usize = 60;
/// How often heartbeat pings are sent
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// How long before lack of client response causes a timeout
const CLIENT_TIMEOUT: Duration = Duration::from_secs(16);

// Input message of ClientSession
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MailboxMessage {
    // messages from client
    JoinRoom { room: String },
    Register { room: String, username: String },
    SendChat { room: String, message: String },
    TakeTurn { room: String },
    ResetTurn { room: String },
    // messages from room
    RoomNotification(room::RoomState),
    ChatNotification(room::ChatNotification),
}

pub type ClientAddr = mpsc::Sender<MailboxMessage>;

// Output message to be sent via websocket
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub enum WsResponse {
    ClientState { id: SessionId },
    Room(room::RoomState),
    Chat(room::ChatNotification),
    Error { error: String },
}

impl Into<Message> for WsResponse {
    fn into(self) -> Message {
        Message::Text(serde_json::to_string(&self).unwrap().into())
    }
}

#[derive(Debug)]
pub struct ClientSession {
    pub id: SessionId,
    pub addr: ClientAddr,
    pub rooms: HashMap<String, RoomAddr>,
}

#[warn(unreachable_patterns)]
fn parse_ws_message(text: &str) -> Result<MailboxMessage, anyhow::Error> {
    let text = std::str::from_utf8(text.as_bytes())?;
    debug!("client sent str: {:?}", text);
    let mailbox_msg = serde_json::from_str::<MailboxMessage>(text)
        .map_err(|_| anyhow::anyhow!("Can't parse json"))?;
    match mailbox_msg {
        msg @ MailboxMessage::SendChat { .. }
        | msg @ MailboxMessage::JoinRoom { .. }
        | msg @ MailboxMessage::TakeTurn { .. }
        | msg @ MailboxMessage::ResetTurn { .. }
        | msg @ MailboxMessage::Register { .. } => Ok(msg),
        _ => {
            anyhow::bail!("Not allow message type: {}", text)
        }
    }
}

async fn handle_ws_messages(
    client_addr: ClientAddr,
    mut ws_receiver: SplitStream<WebSocket>,
    ws_sender: UnboundedSender<Message>,
) -> Result<(), anyhow::Error> {
    let mut last_heartbeat = Instant::now();
    loop {
        match timeout(HEARTBEAT_INTERVAL, ws_receiver.next()).await {
            Ok(msg) => {
                last_heartbeat = Instant::now();
                match msg {
                    Some(Ok(msg)) => match msg {
                        Message::Text(text) => match parse_ws_message(&text) {
                            Ok(mailbox_msg) => client_addr.send(mailbox_msg).await?,
                            Err(err) => {
                                ws_sender.send(
                                    WsResponse::Error {
                                        error: format!("Invalid message: error={err}, msg={text}"),
                                    }
                                    .into(),
                                )?;
                                warn!("Invalid message {}", text);
                            }
                        },
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
                    },
                    Some(Err(_err)) => {
                        debug!("client disconnected");
                        break;
                    }
                    None => break,
                }
            }
            Err(_elapsed) => {
                if Instant::now().duration_since(last_heartbeat) > CLIENT_TIMEOUT {
                    warn!("Websocket Client heartbeat failed, disconnecting!");
                    ws_sender.send(Message::Close(None))?;
                    break;
                } else {
                    ws_sender.send(Message::Ping(b"hb".to_vec()))?;
                }
            }
        }
    }
    debug!("end socket received task");
    Ok(())
}

async fn handle_mailbox_message(
    mut session: ClientSession,
    mut mailbox_rx: Receiver<MailboxMessage>,
    broker_addr: BrokerAddr,
    ws_sender: UnboundedSender<Message>,
) -> Result<(), anyhow::Error> {
    while let Some(msg) = mailbox_rx.recv().await {
        match msg {
            MailboxMessage::JoinRoom { room: name } => {
                let room_addr = {
                    let (send, recv) = oneshot::channel();
                    broker_addr
                        .send(broker::MailboxMessage::JoinRoom {
                            session_id: session.id,
                            room: name.clone(),
                            respond_to: send,
                        })
                        .await?;
                    debug!("created room {}", name);
                    recv.await?
                };
                room_addr
                    .send(room::MailboxMessage::Join {
                        session_id: session.id,
                        client_addr: session.addr.clone(),
                    })
                    .await?;
                debug!("joined room {}", name);
                let _ = session.rooms.insert(name.clone(), room_addr);
            }
            MailboxMessage::Register { room, username } => {
                if let Some(room_addr) = session.rooms.get(&room) {
                    room_addr
                        .send(room::MailboxMessage::Register {
                            session_id: session.id,
                            username: username,
                        })
                        .await?;
                }
            }
            MailboxMessage::SendChat { room, message } => {
                if let Some(room_addr) = session.rooms.get(&room) {
                    room_addr
                        .send(room::MailboxMessage::Chat {
                            session_id: session.id,
                            message: message,
                        })
                        .await?;
                }
            }
            MailboxMessage::TakeTurn { room } => {
                if let Some(room_addr) = session.rooms.get(&room) {
                    room_addr
                        .send(room::MailboxMessage::TakeTurn {
                            session_id: session.id,
                        })
                        .await?;
                }
            }
            MailboxMessage::ResetTurn { room } => {
                if let Some(room_addr) = session.rooms.get(&room) {
                    room_addr
                        .send(room::MailboxMessage::ResetTurn {
                            session_id: session.id,
                        })
                        .await?;
                }
            }
            MailboxMessage::RoomNotification(room) => {
                ws_sender.send(WsResponse::Room(room).into())?;
            }
            MailboxMessage::ChatNotification(chat) => {
                ws_sender.send(WsResponse::Chat(chat).into())?;
            }
        }
    }
    Ok(())
}

// utility to convert sink to sender, as sink can't be cloned
fn sink_to_sender<S, Item>(mut sink: SplitSink<S, Item>) -> UnboundedSender<Item>
where
    SplitSink<S, Item>: SinkExt<Item> + Send + 'static,
    <SplitSink<S, Item> as futures_util::Sink<Item>>::Error: Debug,
    Item: Send + Sync + Debug,
{
    let (tx, mut rx) = mpsc::unbounded_channel::<Item>();

    tokio::task::spawn(async move {
        while let Some(msg) = rx.recv().await {
            debug!("Server sending {:?}", &msg);
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
    client_addr: ClientAddr,
    broker_addr: BrokerAddr,
) -> Result<SessionId, anyhow::Error> {
    let (send, recv) = oneshot::channel();
    let msg = broker::MailboxMessage::Connect {
        client_addr: client_addr,
        respond_to: send,
    };

    broker_addr.send(msg).await?;
    Ok(recv.await?)
}

async fn disconnect(session_id: SessionId, broker_addr: BrokerAddr) -> Result<(), anyhow::Error> {
    let msg = broker::MailboxMessage::Disconnect {
        session_id: session_id,
    };
    broker_addr.send(msg).await?;
    Ok(())
}

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
    let sender = sink_to_sender(socket_sender);

    let (client_addr, mailbox_rx) = mpsc::channel::<MailboxMessage>(CHANNEL_SIZE);
    let session_id = connect(client_addr.clone(), broker_addr.clone()).await?;
    let session = ClientSession {
        id: session_id,
        addr: client_addr.clone(),
        rooms: HashMap::new(),
    };

    let mut mailbox_recv_task = spawn_and_log_error(handle_mailbox_message(
        session,
        mailbox_rx,
        broker_addr.clone(),
        sender.clone(),
    ));

    let mut socket_recv_task = spawn_and_log_error(handle_ws_messages(
        client_addr,
        socket_receiver,
        sender.clone(),
    ));
    debug!("spawned socket_recv_task and mailbox_recv_task");

    // If any one of the tasks exit, abort the other.
    tokio::select! {
        _ = (&mut mailbox_recv_task) => socket_recv_task.abort(),
        _ = (&mut socket_recv_task) => mailbox_recv_task.abort(),
    };

    // no need to tell the room, as if client quit, room will automatically remove it
    disconnect(session_id, broker_addr).await?;
    debug!("done, closing");
    Ok(())
}
