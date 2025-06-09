use std::fmt::Debug;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use sudo::RunningAs;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tokio::sync::mpsc::{Receiver, Sender};

use super::BotAPI;

pub struct Cli<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    event_tx: Sender<super::Event<C>>,
    event_rx: Arc<Mutex<Receiver<super::Event<C>>>>,

    context: C,
}

impl<C> Cli<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    #[must_use]
    pub fn new(context: C) -> Self
    where
        C: Clone + Debug + Send + Sync + 'static,
    {
        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<super::Event<C>>(1);

        Self {
            event_tx,
            event_rx: Arc::new(Mutex::new(event_rx)),

            context,
        }
    }

    async fn handle_line(self: &Arc<Self>, line: Option<String>) {
        let Some(line) = line else { return };

        let sender = crate::User::new(users::get_current_uid().to_string()).nickname(
            users::get_current_username()
                .unwrap_or_default()
                .into_string()
                .unwrap_or_default(),
        );
        if let Err(err) = self
            .event_tx
            .send(super::Event::Message(crate::Message::new(
                now_as_id(),
                super::MessageContents::new().text(line),
                super::Chat::private(self.clone(), sender.clone()),
                sender,
            )))
            .await
        {
            tracing::error!("{err:?}");
        }
    }
}

#[async_trait::async_trait]
impl<C> BotAPI<C> for Cli<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    fn get_context(&self) -> &C {
        &self.context
    }

    async fn run(self: Arc<Self>) {
        let mut reader = BufReader::new(tokio::io::stdin()).lines();
        loop {
            match reader.next_line().await {
                Ok(line) => self.handle_line(line).await,
                Err(err) => {
                    tracing::error!("{err:?}");
                    break;
                },
            }
        }
    }

    async fn next_event(&self) -> Option<super::Event<C>> {
        let mut events = self.event_rx.lock().await;
        events.recv().await
    }

    async fn send_msg_inner(
        &self,
        contents: super::MessageContents,
        _: super::Chat<C>,
    ) -> Result<String> {
        println!("```\n{contents}\n```");

        Ok(now_as_id())
    }

    async fn is_group_admin(&self, _: &crate::User, _: &crate::Group) -> Result<bool> {
        Ok(sudo::check() == RunningAs::Root)
    }
}

fn now_as_id() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |t| t.as_millis().try_into().unwrap_or(0))
        .to_string()
}
