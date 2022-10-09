use crate::{
    actor::{Actor, Mailbox},
    room::{Room, RoomAddr, WeakRoomAddr},
    user_session::{SessionId, UserSessionAddr},
};
use serde::{Deserialize, Serialize};
use std::collections::{hash_map::Entry, HashMap};
use tokio::sync::{
    mpsc::{self},
    oneshot,
};
use tracing::log::{debug, warn};

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct BrokerStats {
    pub room_count: usize,
    pub session_count: usize,
}

#[derive(Debug)]
pub enum MailboxMessage {
    Connect {
        client_addr: UserSessionAddr,
        respond_to: oneshot::Sender<SessionId>,
    },
    Disconnect {
        session_id: SessionId,
    },
    JoinRoom {
        session_id: SessionId,
        room: String,
        respond_to: oneshot::Sender<RoomAddr>,
    },
    CloseRoom {
        room: String,
    },
    Stats {
        respond_to: oneshot::Sender<BrokerStats>,
    },
}

pub type BrokerAddr = mpsc::Sender<MailboxMessage>;

#[derive(Debug)]
pub struct Broker {
    max_session_id: SessionId,
    sessions: HashMap<SessionId, UserSessionAddr>,
    rooms: HashMap<String, WeakRoomAddr>,
}

#[async_trait::async_trait]
impl Actor for Broker {
    type MailboxMessage = MailboxMessage;
    const CHANNEL_SIZE: usize = 100;

    async fn run(mut self: Self, mut mailbox: Mailbox<Self::MailboxMessage>) -> anyhow::Result<()> {
        use MailboxMessage::*;
        while let Some(message) = mailbox.receiver.recv().await {
            match message {
                Connect {
                    client_addr,
                    respond_to,
                } => {
                    self.max_session_id += 1;
                    self.sessions.insert(self.max_session_id, client_addr);
                    if let Err(err) = respond_to.send(self.max_session_id) {
                        warn!("Error on client connect: {}", err)
                    }
                }
                Disconnect { session_id } => {
                    let _ = self.sessions.remove(&session_id);
                }
                JoinRoom {
                    session_id: _,
                    room: name,
                    respond_to,
                } => {
                    let room_addr_opt: Option<RoomAddr> = match self.rooms.entry(name.clone()) {
                        Entry::Occupied(entry) => {
                            debug!("broker: Found room, reuse");
                            entry.get().clone().upgrade()
                        }
                        Entry::Vacant(_vacant) => None,
                    };
                    if room_addr_opt.is_none() {
                        match mailbox.sender() {
                            Some(sender) => {
                                let room_addr = Room::new(name.clone(), sender).spawn();
                                debug!("broker: Room created");
                                let _ = self.rooms.insert(name, room_addr.downgrade());
                                let _ = respond_to.send(room_addr);
                            }
                            None => return Ok(()), // channel is dropped, quit
                        }
                    } else {
                        let _ = respond_to.send(room_addr_opt.unwrap());
                    }
                }
                CloseRoom { room: name } => {
                    let room_addr_opt: Option<RoomAddr> = match self.rooms.entry(name.clone()) {
                        Entry::Occupied(entry) => {
                            debug!("broker: Found room, reuse");
                            entry.get().clone().upgrade()
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

impl Broker {
    pub fn new() -> Broker {
        Broker {
            max_session_id: 0,
            sessions: HashMap::new(),
            rooms: HashMap::new(),
        }
    }
}
