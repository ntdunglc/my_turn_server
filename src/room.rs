use crate::{
    actor::{Actor, Mailbox},
    broker::{self, BrokerAddr},
    user_session::{self, SessionId, UserSessionAddr},
};
use futures::TryFutureExt;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc::{self};
use tracing::log::debug;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoomState {
    pub name: String,
    pub users: HashSet<String>, // only registered users appear here
    pub turns: Vec<String>,     // list of usernames, sorted by order they take this turn
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatNotification {
    pub room: String,
    pub username: String,
    pub message: String,
}

#[derive(Debug)]
pub enum MailboxMessage {
    Join {
        session_id: SessionId,
        client_addr: UserSessionAddr,
    }, // client can just join and watch
    // user needs to register with an room's username before they can participate
    Chat {
        session_id: SessionId,
        message: String,
    },
    Register {
        session_id: SessionId,
        username: String,
    },
    TakeTurn {
        session_id: SessionId,
    },
    ResetTurn {
        session_id: SessionId,
    },
}
pub type RoomAddr = mpsc::Sender<MailboxMessage>;
pub type WeakRoomAddr = mpsc::WeakSender<MailboxMessage>;

#[derive(Debug)]
pub struct Room {
    state: RoomState,
    broker_addr: BrokerAddr,
    sessions: HashMap<SessionId, UserSessionAddr>,
    registered_users: HashMap<SessionId, String>,
}

#[async_trait::async_trait]
impl Actor for Room {
    type MailboxMessage = MailboxMessage;

    async fn run(mut self: Self, mut mailbox: Mailbox<Self::MailboxMessage>) -> anyhow::Result<()> {
        debug!("room: Room created");
        while let Some(msg) = mailbox.receiver.recv().await {
            self.handle_message(msg).await;
        }
        debug!("Room closing");
        let _ = self
            .broker_addr
            .send(broker::MailboxMessage::CloseRoom {
                room: self.state.name.clone(),
            })
            .await;
        Ok(())
    }
}

impl Room {
    pub fn new(name: String, broker_addr: BrokerAddr) -> Room {
        Room {
            state: RoomState {
                name,
                users: HashSet::new(),
                turns: vec![],
            },
            broker_addr,
            sessions: HashMap::new(),
            registered_users: HashMap::new(),
        }
    }

    pub async fn handle_message(&mut self, message: MailboxMessage) {
        use MailboxMessage::*;
        match message {
            Join {
                session_id,
                client_addr,
            } => {
                let _ = client_addr
                    .send(user_session::MailboxMessage::RoomNotification(
                        self.state.clone(),
                    ))
                    .await;
                let _ = self.sessions.insert(session_id, client_addr);
            }
            Register {
                session_id,
                username,
            } => {
                if self.sessions.contains_key(&session_id) {
                    // todo: check username uniqueness
                    self.registered_users.insert(session_id, username.clone());
                    self.state.users.insert(username);
                    self.broadcast_state().await;
                }
            }
            Chat {
                session_id,
                message,
            } => {
                if let Some(username) = self.registered_users.get(&session_id) {
                    self.broadcast_chat(username.clone(), message).await;
                }
            }
            TakeTurn { session_id } => {
                if let Some(username) = self.registered_users.get(&session_id) {
                    if !self.state.turns.contains(username) {
                        self.state.turns.push(username.to_string());
                    }
                    self.broadcast_state().await;
                }
            }
            ResetTurn { session_id } => {
                if let Some(_username) = self.registered_users.get(&session_id) {
                    self.state.turns.clear();
                    self.broadcast_state().await;
                }
            }
        }
    }

    async fn broadcast_chat(&mut self, username: String, message: String) {
        self.broadcast(user_session::MailboxMessage::ChatNotification(
            ChatNotification {
                room: self.state.name.clone(),
                username: username,
                message: message,
            },
        ))
        .await;
    }

    async fn broadcast_state(&mut self) {
        self.broadcast(user_session::MailboxMessage::RoomNotification(
            self.state.clone(),
        ))
        .await;
    }

    // broadcast will remove closed sessions
    async fn broadcast(&mut self, message: user_session::MailboxMessage) {
        let mut handles = vec![];
        for (session_id, client_addr) in self.sessions.iter() {
            handles.push(
                client_addr
                    .send(message.clone())
                    .map_err(|err| (session_id.clone(), err)),
            );
        }
        for res in futures::future::join_all(handles).await {
            if let Err((session_id, _err)) = res {
                let _ = self.sessions.remove(&session_id);
            }
        }
    }
}
