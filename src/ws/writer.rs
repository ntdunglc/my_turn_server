use crate::{
    actor::{Actor, Mailbox},
    room::{self},
};
use axum::extract::ws::{Message, WebSocket};
use futures_util::{stream::SplitSink, SinkExt};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

use tracing::log::{debug, warn};

// Output message to be sent via websocket
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub enum MailboxMessage {
    Room(room::RoomState),
    Chat(room::ChatNotification),
    Error { error: String },
}

impl Into<Message> for MailboxMessage {
    fn into(self) -> Message {
        Message::Text(serde_json::to_string(&self).unwrap().into())
    }
}
#[derive(Debug)]
pub struct WsWriter {
    pub sink: SplitSink<WebSocket, Message>,
}

#[async_trait::async_trait]
impl Actor for WsWriter {
    type MailboxMessage = Message;

    async fn run(mut self: Self, mut mailbox: Mailbox<Self::MailboxMessage>) -> anyhow::Result<()> {
        debug!("room: Room created");
        while let Some(msg) = mailbox.receiver.recv().await {
            // In any websocket error, break loop.
            if let Err(e) = self.sink.send(msg.into()).await {
                warn!("websocket send error: {:?}", e);
                break;
            }
        }
        debug!("Closing WsWriter");
        Ok(())
    }
}
