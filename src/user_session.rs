use crate::{
    actor::{Actor, Mailbox},
    broker::{self, BrokerAddr},
    room::{self, RoomAddr},
    ws,
};
use anyhow::Context;
use axum::extract::ws::Message;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt::Debug};
use tokio::sync::{
    mpsc::{self, Sender},
    oneshot,
};
use tracing::log::debug;
pub use usize as SessionId;

// Input message of ClientSession
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MailboxMessage {
    // messages from websocket
    WsMessage(ws::reader::MailboxMessage),
    // messages from room
    RoomNotification(room::RoomState),
    ChatNotification(room::ChatNotification),
    // WS session close
    Close,
}

pub type UserSessionAddr = mpsc::Sender<MailboxMessage>;
pub type UserSessionAddrWeak = mpsc::WeakSender<MailboxMessage>;

#[derive(Debug)]
pub struct UserSession {
    pub id: SessionId,
    pub rooms: HashMap<String, RoomAddr>,
    pub broker_addr: BrokerAddr,
    pub ws_writer_addr: Sender<Message>,
}

#[async_trait::async_trait]
impl Actor for UserSession {
    type MailboxMessage = MailboxMessage;

    async fn run(mut self: Self, mut mailbox: Mailbox<Self::MailboxMessage>) -> anyhow::Result<()> {
        self.id = self
            .connect(mailbox.sender().context("Session terminated")?)
            .await?;
        while let Some(msg) = mailbox.receiver.recv().await {
            match msg {
                MailboxMessage::WsMessage(ws::reader::MailboxMessage::JoinRoom { room: name }) => {
                    let room_addr = {
                        let (send, recv) = oneshot::channel();
                        self.broker_addr
                            .send(broker::MailboxMessage::JoinRoom {
                                session_id: self.id,
                                room: name.clone(),
                                respond_to: send,
                            })
                            .await?;
                        debug!("created room {}", name);
                        recv.await?
                    };
                    room_addr
                        .send(room::MailboxMessage::Join {
                            session_id: self.id,
                            client_addr: mailbox.sender().context("Session terminated")?.clone(),
                        })
                        .await?;
                    debug!("joined room {}", name);
                    let _ = self.rooms.insert(name.clone(), room_addr);
                }
                MailboxMessage::WsMessage(ws::reader::MailboxMessage::Register {
                    room,
                    username,
                }) => {
                    if let Some(room_addr) = self.rooms.get(&room) {
                        room_addr
                            .send(room::MailboxMessage::Register {
                                session_id: self.id,
                                username: username,
                            })
                            .await?;
                    }
                }
                MailboxMessage::WsMessage(ws::reader::MailboxMessage::SendChat {
                    room,
                    message,
                }) => {
                    if let Some(room_addr) = self.rooms.get(&room) {
                        room_addr
                            .send(room::MailboxMessage::Chat {
                                session_id: self.id,
                                message: message,
                            })
                            .await?;
                    }
                }
                MailboxMessage::WsMessage(ws::reader::MailboxMessage::TakeTurn { room }) => {
                    if let Some(room_addr) = self.rooms.get(&room) {
                        room_addr
                            .send(room::MailboxMessage::TakeTurn {
                                session_id: self.id,
                            })
                            .await?;
                    }
                }
                MailboxMessage::WsMessage(ws::reader::MailboxMessage::ResetTurn { room }) => {
                    if let Some(room_addr) = self.rooms.get(&room) {
                        room_addr
                            .send(room::MailboxMessage::ResetTurn {
                                session_id: self.id,
                            })
                            .await?;
                    }
                }
                MailboxMessage::RoomNotification(room) => {
                    self.ws_writer_addr
                        .send(ws::writer::MailboxMessage::Room(room).into())
                        .await?;
                }
                MailboxMessage::ChatNotification(chat) => {
                    self.ws_writer_addr
                        .send(ws::writer::MailboxMessage::Chat(chat).into())
                        .await?;
                }
                MailboxMessage::Close => break,
            }
        }
        self.disconnect().await?;
        Ok(())
    }
}

impl UserSession {
    pub fn new(broker_addr: BrokerAddr, ws_writer_addr: Sender<Message>) -> UserSession {
        UserSession {
            id: 0,
            rooms: HashMap::new(),
            broker_addr,
            ws_writer_addr,
        }
    }
    async fn connect(&mut self, client_addr: UserSessionAddr) -> Result<SessionId, anyhow::Error> {
        let (send, recv) = oneshot::channel();
        let msg = broker::MailboxMessage::Connect {
            client_addr: client_addr,
            respond_to: send,
        };

        self.broker_addr.send(msg).await?;
        Ok(recv.await?)
    }

    async fn disconnect(&mut self) -> Result<(), anyhow::Error> {
        let msg = broker::MailboxMessage::Disconnect {
            session_id: self.id,
        };
        self.broker_addr.send(msg).await?;
        Ok(())
    }
}
