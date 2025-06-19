use std::fmt::Debug;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use sudo::RunningAs;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;
use tokio::sync::mpsc::{Receiver, Sender};

use crate::BotAPI;

static DEFAULT_BOT_ID: &str = "-";

pub struct Cli<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    event_tx: Sender<crate::Event<C>>,
    event_rx: Arc<Mutex<Receiver<crate::Event<C>>>>,

    self_user: crate::User,

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
        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<crate::Event<C>>(1);

        Self {
            self_user: crate::User::new(DEFAULT_BOT_ID.to_owned()),

            event_tx,
            event_rx: Arc::new(Mutex::new(event_rx)),

            context,
        }
    }

    async fn handle_line(self: &Arc<Self>, line: Option<String>) {
        let Some(line) = line else { return };

        let be_at_text = format!("@{} ", self.self_user.id);

        let positions: Vec<_> = line.match_indices(&be_at_text).collect();

        let mut contents = crate::MessageContents::new();
        let mut last_pos = 0;
        for (pos, _) in positions {
            if pos > last_pos {
                contents = contents.text(&line[last_pos..pos]);
            }

            contents = contents.at(crate::User::new(self.self_user.id.clone()));
            last_pos = pos + be_at_text.len();
        }

        let sender = crate::User::new(users::get_current_uid().to_string()).nickname(
            users::get_current_username()
                .unwrap_or_default()
                .into_string()
                .unwrap_or_default(),
        );

        let msg = crate::Message::new(
            now_as_id(),
            contents,
            crate::Chat::private(self.clone(), sender.clone()),
            sender,
        );

        if let Err(err) = self.event_tx.send(crate::Event::Message(msg)).await {
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

    fn get_self_user(&self) -> &crate::User {
        &self.self_user
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

    async fn next_event(&self) -> Option<crate::Event<C>> {
        let mut events = self.event_rx.lock().await;
        events.recv().await
    }

    async fn send_msg_inner(
        &self,
        contents: crate::MessageContents,
        _: crate::Chat<C>,
        _: Option<&crate::Message<C>>,
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
