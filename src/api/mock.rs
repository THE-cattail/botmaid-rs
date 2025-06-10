use std::fmt::{Debug, Display};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{Receiver, Sender};

use crate::BotAPI;

pub static DEFAULT_BOT_ID: &str = "-";
pub static DEFAULT_SENDER_ID: &str = "0";
pub static DEFAULT_SENDER_NICKNAME: &str = "tester";

pub struct Mock<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    event_tx: Sender<crate::Event<C>>,
    event_rx: Arc<Mutex<Receiver<crate::Event<C>>>>,

    actions: Arc<Mutex<Vec<Action<C>>>>,

    self_user: crate::User,

    context: C,
}

impl<C> Mock<C>
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

            actions: Arc::new(Mutex::new(Vec::new())),

            context,
        }
    }

    pub fn happen(self: &Arc<Self>, event: crate::Event<C>) {
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
        self.happen(crate::Event::Message(crate::Message::new(
            uuid::Uuid::new_v4().to_string(),
            contents,
            crate::Chat::private(self.clone(), sender.clone()),
            sender,
        )));
    }

    pub fn simple_text<D>(self: &Arc<Self>, text: D)
    where
        D: Display,
    {
        self.simple_msg(crate::MessageContents::new().text(text));
    }

    pub async fn get_actions(&self) -> Vec<Action<C>> {
        self.get_actions_with_timeout(Duration::from_millis(200))
            .await
    }

    pub async fn get_actions_with_timeout(&self, timeout: Duration) -> Vec<Action<C>> {
        tokio::time::sleep(timeout).await;
        self.actions.lock().await.drain(..).collect()
    }
}

#[async_trait::async_trait]
impl<C> BotAPI<C> for Mock<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    fn get_context(&self) -> &C {
        &self.context
    }

    fn get_self_user(&self) -> &crate::User {
        &self.self_user
    }

    async fn run(self: Arc<Self>) {}

    async fn next_event(&self) -> Option<crate::Event<C>> {
        let mut events = self.event_rx.lock().await;
        events.recv().await
    }

    async fn send_msg_inner(
        &self,
        contents: crate::MessageContents,
        chat: crate::Chat<C>,
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

    async fn is_group_admin(&self, user: &crate::User, _: &crate::Group) -> Result<bool> {
        Ok(user.id == DEFAULT_SENDER_ID)
    }
}

#[derive(Clone)]
pub enum Action<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    SendMessage(crate::Message<C>),
    #[allow(dead_code)]
    Other,
}

impl<C> Action<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    #[must_use]
    pub fn send_msg(self) -> Option<crate::Message<C>> {
        if let Self::SendMessage(msg) = self {
            Some(msg)
        } else {
            None
        }
    }
}
