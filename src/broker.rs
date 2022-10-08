use crate::{
    room::{self, Room, RoomAddr, WeakRoomAddr},
    utils::spawn_and_log_error,
    ws::{self, ClientAddr, SessionId},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{hash_map::Entry, HashMap, HashSet},
    sync::Arc,
};
use tokio::{
    sync::{
        mpsc::{self, Receiver},
        oneshot,
    },
    task::JoinHandle,
};
use tracing::log::{debug, warn};

const CHANNEL_SIZE: usize = 60;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct BrokerStats {
    pub room_count: usize,
    pub session_count: usize,
}

#[derive(Debug)]
pub enum MailboxMessage {
    Connect {
        client_addr: ClientAddr,
        respond_to: oneshot::Sender<SessionId>,
    },
    Disconnect {
        session_id: SessionId,
    },
    CreateRoom {
        session_id: SessionId,
        name: String,
        respond_to: oneshot::Sender<RoomAddr>,
    },
    CloseRoom {
        name: String,
    },
    Stats {
        respond_to: oneshot::Sender<BrokerStats>,
    },
}

pub type BrokerAddr = mpsc::Sender<MailboxMessage>;

#[derive(Debug)]
pub struct RoomState {
    room_addr: WeakRoomAddr,
    handle: JoinHandle<()>,
}
#[derive(Debug)]
pub struct SessionState {
    client_addr: ClientAddr,
}

#[derive(Debug)]
pub struct Broker {
    max_session_id: SessionId,
    sessions: HashMap<SessionId, SessionState>,
    rooms: HashMap<String, RoomState>,
    addr: BrokerAddr,
}

impl Broker {
    pub fn spawn() -> (BrokerAddr, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel::<MailboxMessage>(CHANNEL_SIZE);
        let broker = Broker {
            max_session_id: 0,
            sessions: HashMap::new(),
            rooms: HashMap::new(),
            addr: tx.clone(),
        };
        let handle = spawn_and_log_error(broker.handle_messages(rx));
        (tx, handle)
    }

    pub async fn handle_messages(mut self, mut rx: Receiver<MailboxMessage>) -> anyhow::Result<()> {
        use MailboxMessage::*;
        // server should never stop
        while let Some(message) = rx.recv().await {
            match message {
                Connect {
                    client_addr,
                    respond_to,
                } => {
                    self.max_session_id += 1;
                    self.sessions
                        .insert(self.max_session_id, SessionState { client_addr });
                    if let Err(err) = respond_to.send(self.max_session_id) {
                        warn!("Error on client connect: {}", err)
                    }
                }
                Disconnect { session_id } => {
                    let _ = self.sessions.remove(&session_id);
                }
                CreateRoom {
                    session_id,
                    name,
                    respond_to,
                } => {
                    let room_addr_opt: Option<RoomAddr> = match self.rooms.entry(name.clone()) {
                        Entry::Occupied(entry) => {
                            debug!("broker: Found room, reuse");
                            entry.get().room_addr.clone().upgrade()
                        }
                        Entry::Vacant(vacant) => None,
                    };
                    if room_addr_opt.is_none() {
                        let (room_addr, handle) = Room::spawn(name.clone(), self.addr.clone());
                        debug!("broker: Room created");
                        let room_state = RoomState {
                            room_addr: room_addr.downgrade(),
                            handle,
                        };
                        let _ = self.rooms.insert(name, room_state);
                        let _ = respond_to.send(room_addr);
                    } else {
                        let _ = respond_to.send(room_addr_opt.unwrap());
                    }
                }
                CloseRoom { name } => {
                    let room_addr_opt: Option<RoomAddr> = match self.rooms.entry(name.clone()) {
                        Entry::Occupied(entry) => {
                            debug!("broker: Found room, reuse");
                            entry.get().room_addr.clone().upgrade()
                        }
                        Entry::Vacant(_) => None,
                    };
                    debug!("Removing room: {}", &name);
                    // we only remove a room if no sender is holding it
                    if room_addr_opt.is_none() {
                        let _ = self.rooms.remove(&name);
                    }
                }
                Stats { respond_to } => {
                    let _ = respond_to.send(BrokerStats {
                        room_count: self.rooms.len(),
                        session_count: self.sessions.len(),
                    });
                }
            }
        }
        Ok(())
    }
}
