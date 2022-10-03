use crate::ws::{self, SessionId};
use std::collections::{HashMap, HashSet};
use tokio::sync::{mpsc, oneshot};
use tracing::log::warn;

const CHANNEL_SIZE: usize = 60;

#[derive(Debug)]
pub enum MailboxMessage {
    Connect {
        respond_to: oneshot::Sender<SessionId>,
    },
    Disconnect {
        session_id: SessionId,
    },
}

#[derive(Debug)]
pub struct Server {
    max_session_id: SessionId,
    sessions: HashMap<SessionId, mpsc::Sender<ws::MailboxMessage>>,
    // rooms: HashMap<String, HashSet<SessionId>>,
}

impl Server {
    pub fn start() -> mpsc::Sender<MailboxMessage> {
        let mut server = Server {
            max_session_id: 0,
            sessions: HashMap::new(),
            // rooms: HashMap::new(),
        };
        let (tx, mut rx) = mpsc::channel::<MailboxMessage>(CHANNEL_SIZE);
        tokio::task::spawn(async move {
            while let Some(msg) = rx.recv().await {
                match msg {
                    MailboxMessage::Connect { respond_to } => {
                        server.max_session_id += 1;
                        if let Err(err) = respond_to.send(server.max_session_id) {
                            warn!("Error on client connect: {}", err)
                        }
                    }
                    MailboxMessage::Disconnect { session_id } => {}
                }
            }
        });
        tx
    }
}
