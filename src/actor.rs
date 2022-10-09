use crate::utils::spawn_and_log_error;
use async_trait::async_trait;
use tokio::sync::mpsc::{self, Receiver, Sender, WeakSender};

pub struct Mailbox<T> {
    // store weak_sender to make sure that if all other senders are dropped
    // this actor will finish. A strong sender will keep the channel open.
    pub weak_sender: WeakSender<T>,
    pub receiver: Receiver<T>,
}

impl<T> Mailbox<T> {
    // this function should only be called when the channel is not closed
    pub fn sender(&self) -> Option<Sender<T>> {
        self.weak_sender.upgrade()
    }
}

#[async_trait]
pub trait Actor {
    type MailboxMessage;
    const CHANNEL_SIZE: usize = 30;

    fn spawn(self: Self) -> mpsc::Sender<Self::MailboxMessage>
    where
        Self: Sized + 'static,
    {
        let (tx, rx) = mpsc::channel::<Self::MailboxMessage>(Self::CHANNEL_SIZE);
        let _ = spawn_and_log_error(self.run(Mailbox {
            weak_sender: tx.clone().downgrade(),
            receiver: rx,
        }));
        tx
    }

    async fn run(mut self: Self, mut rx: Mailbox<Self::MailboxMessage>) -> anyhow::Result<()>;
}
