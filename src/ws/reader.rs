use crate::{
    actor::{Actor, Mailbox},
    user_session,
    ws::writer,
};
use axum::extract::ws::{Message, WebSocket};
use futures_util::{stream::SplitStream, StreamExt};
use serde::{Deserialize, Serialize};
use std::{fmt::Debug, time::Duration};
use tokio::{
    sync::mpsc::Sender,
    time::{timeout, Instant},
};
use tracing::log::{debug, warn};

/// How often heartbeat pings are sent
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// How long before lack of client response causes a timeout
const CLIENT_TIMEOUT: Duration = Duration::from_secs(16);

// Output message to be sent via websocket
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum MailboxMessage {
    JoinRoom { room: String },
    Register { room: String, username: String },
    SendChat { room: String, message: String },
    TakeTurn { room: String },
    ResetTurn { room: String },
}

#[derive(Debug)]
pub struct WsReader {
    pub receiver: SplitStream<WebSocket>,
    pub user_session_addr: user_session::UserSessionAddr,
    pub ws_writer_addr: Sender<Message>,
}

#[async_trait::async_trait]
impl Actor for WsReader {
    type MailboxMessage = ();

    async fn run(mut self: Self, _: Mailbox<Self::MailboxMessage>) -> anyhow::Result<()> {
        let mut last_heartbeat = Instant::now();
        loop {
            match timeout(HEARTBEAT_INTERVAL, self.receiver.next()).await {
                Ok(msg) => {
                    last_heartbeat = Instant::now();
                    match msg {
                        Some(Ok(msg)) => match msg {
                            Message::Text(text) => match Self::parse_ws_message(&text) {
                                Ok(ws_msg) => {
                                    self.user_session_addr
                                        .send(user_session::MailboxMessage::WsMessage(ws_msg))
                                        .await?
                                }
                                Err(err) => {
                                    self.ws_writer_addr
                                        .send(
                                            writer::MailboxMessage::Error {
                                                error: format!(
                                                    "Invalid message: error={err}, msg={text}"
                                                ),
                                            }
                                            .into(),
                                        )
                                        .await?;
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
                        self.ws_writer_addr.send(Message::Close(None)).await?;
                        break;
                    } else {
                        self.ws_writer_addr
                            .send(Message::Ping(b"hb".to_vec()))
                            .await?;
                    }
                }
            }
        }
        self.user_session_addr
            .send(user_session::MailboxMessage::Close)
            .await?;
        debug!("Closing WsReader");
        Ok(())
    }
}

impl WsReader {
    #[warn(unreachable_patterns)]
    fn parse_ws_message(text: &str) -> anyhow::Result<MailboxMessage> {
        let text = std::str::from_utf8(text.as_bytes())?;
        debug!("client sent str: {:?}", text);
        serde_json::from_str::<MailboxMessage>(text)
            .map_err(|_| anyhow::anyhow!("Can't parse json"))
    }
}
