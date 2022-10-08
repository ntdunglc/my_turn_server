use axum::Json;
use tokio::sync::oneshot;

use crate::{
    broker::{self, BrokerAddr},
    error::AppError,
};

pub async fn handler(broker_addr: BrokerAddr) -> Result<Json<broker::BrokerStats>, AppError> {
    let (send, recv) = oneshot::channel();
    broker_addr
        .send(broker::MailboxMessage::Stats { respond_to: send })
        .await?;
    Ok(Json(recv.await?))
}
