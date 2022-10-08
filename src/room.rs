use crate::{
    broker::{self, BrokerAddr},
    ws::{self, ClientAddr, SessionId},
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tracing::log::{debug, warn};

const CHANNEL_SIZE: usize = 60;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct RoomState {
    pub name: String,
}

#[derive(Debug)]
pub enum MailboxMessage {
    Join {
        client_addr: ClientAddr,
        name: String,
        respond_to: oneshot::Sender<RoomState>,
    },
}
pub type RoomAddr = mpsc::Sender<MailboxMessage>;
pub type WeakRoomAddr = mpsc::WeakSender<MailboxMessage>;

#[derive(Debug)]
pub struct Room {
    name: String,
    broker_addr: BrokerAddr,
    sessions: HashMap<String, ClientAddr>,
}

impl Room {
    pub fn spawn(name: String, broker_addr: BrokerAddr) -> (RoomAddr, JoinHandle<()>) {
        let mut room = Room {
            name,
            broker_addr,
            sessions: HashMap::new(),
        };
        let (tx, mut rx) = mpsc::channel::<MailboxMessage>(CHANNEL_SIZE);
        let handle = tokio::task::spawn(async move {
            debug!("room: Room created");
            while let Some(msg) = rx.recv().await {
                room.handle_message(msg).await;
            }
            debug!("Room closing");
            let _ = room
                .broker_addr
                .send(broker::MailboxMessage::CloseRoom {
                    name: room.name.clone(),
                })
                .await;
        });
        (tx, handle)
    }

    pub async fn handle_message(&mut self, message: MailboxMessage) {
        use MailboxMessage::*;
        match message {
            Join {
                name,
                client_addr,
                respond_to,
            } => {
                let _ = self.sessions.insert(name, client_addr);
                let _ = respond_to.send(self.get_room_state());
            }
        }
    }

    fn get_room_state(&self) -> RoomState {
        RoomState {
            name: self.name.clone(),
        }
    }
}
