use std::fmt::Display;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{Receiver, Sender};

use super::BotAPI;

pub static DEFAULT_BOT_NAME: &str = "mock";
pub static DEFAULT_SENDER_ID: &str = "0";
pub static DEFAULT_SENDER_NICKNAME: &str = "tester";

pub struct Mock {
    self_user: crate::User,

    event_tx: Sender<crate::Event>,
    event_rx: Arc<Mutex<Receiver<crate::Event>>>,

    actions: Arc<Mutex<Vec<Action>>>,
}

impl Mock {
    #[must_use]
    pub fn new() -> Arc<Self> {
        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<crate::Event>(1);

        Arc::new(Self {
            self_user: crate::User::new(DEFAULT_SENDER_ID.to_owned()),

            event_tx,
            event_rx: Arc::new(Mutex::new(event_rx)),

            actions: Arc::new(Mutex::new(Vec::new())),
        })
    }

    pub fn happen(self: &Arc<Self>, event: crate::Event) {
        let self_clone = self.clone();
        tokio::spawn(async move {
            if let Err(err) = self_clone.event_tx.send(event).await {
                tracing::error!("{err:?}");
            }
        });
    }

    pub fn simple_msg(self: &Arc<Self>, contents: crate::MessageContents) {
        let sender = crate::User::new(DEFAULT_SENDER_ID.to_owned())
            .nickname(DEFAULT_SENDER_NICKNAME.to_owned());
        self.happen(crate::Event::Message(
            crate::Message::new(
                uuid::Uuid::new_v4().to_string(),
                contents,
                crate::Chat::Private(sender.clone()),
                sender,
            )
            .bot(self.clone()),
        ));
    }

    pub fn simple_text<D>(self: &Arc<Self>, text: D)
    where
        D: Display,
    {
        self.simple_msg(crate::MessageContents::new().text(text));
    }

    pub async fn get_actions(&self) -> Vec<Action> {
        self.get_actions_with_timeout(Duration::from_millis(200))
            .await
    }

    pub async fn get_actions_with_timeout(&self, timeout: Duration) -> Vec<Action> {
        tokio::time::sleep(timeout).await;
        self.actions.lock().await.drain(..).collect()
    }
}

#[async_trait::async_trait]
impl BotAPI for Mock {
    async fn run(self: Arc<Self>) {}

    async fn next_event(&self) -> Option<crate::Event> {
        let mut events = self.event_rx.lock().await;
        events.recv().await
    }

    async fn send_msg_inner(
        &self,
        contents: crate::MessageContents,
        chat: crate::Chat,
    ) -> Result<String> {
        self.actions
            .lock()
            .await
            .push(Action::SendMessage(crate::Message::new(
                uuid::Uuid::new_v4().to_string(),
                contents,
                chat,
                self.self_user.clone(),
            )));

        Ok(DEFAULT_SENDER_ID.to_owned())
    }
}

#[derive(Clone)]
pub enum Action {
    SendMessage(crate::Message),
    #[allow(dead_code)]
    Other,
}

impl Action {
    #[must_use]
    pub fn send_msg(self) -> Option<crate::Message> {
        if let Self::SendMessage(msg) = self {
            Some(msg)
        } else {
            None
        }
    }
}
