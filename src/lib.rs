use std::fmt::Display;
use std::sync::{Arc, RwLock};

use anyhow::Result;
use api::BotAPI;
use derivative::Derivative;
use tokio::task::JoinSet;

pub mod api;

#[async_trait::async_trait]
pub trait BotInstance: 'static {
    fn api(&self) -> Arc<dyn BotAPI>;

    async fn run(self: Arc<Self>) {
        let self_clone = self.clone();
        tokio::spawn(async move {
            self_clone.api().run().await;
        });

        let self_clone = self.clone();
        tokio::spawn(async move {
            if let Err(err) = self_clone.run_jobs().await {
                tracing::error!("{err:?}");
            }
        });

        while let Some(event) = self.api().next_event().await {
            let self_clone = self.clone();
            tokio::spawn(async move {
                if let Err(err) = self_clone.handle_event(event).await {
                    tracing::error!("{err:?}");
                }
            });
        }
    }

    async fn handle_event(self: Arc<Self>, event: Event) -> Result<()> {
        tracing::info!("handling event: {event:?}");

        match event {
            Event::Message(msg) => self.handle_msg(msg).await,
            Event::Other(_) => Ok(()),
        }
    }

    async fn handle_msg(self: &Arc<Self>, msg: Message) -> Result<()>;
    async fn run_jobs(self: &Arc<Self>) -> Result<()>;
}

pub struct BotMaid<B>
where
    B: BotInstance + Send + Sync,
{
    bots: RwLock<Vec<Arc<B>>>,
}

impl<B> BotMaid<B>
where
    B: BotInstance + Send + Sync,
{
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            bots: RwLock::new(Vec::new()),
        })
    }

    /// # Panics
    pub fn add_bot(self: &Arc<Self>, bot: B) -> Arc<B> {
        let bot = Arc::new(bot);
        self.bots.write().unwrap().push(bot.clone());
        bot
    }

    /// # Panics
    pub async fn run(self: Arc<Self>) {
        let mut join_set = JoinSet::new();
        for bot in &*self.bots.read().unwrap() {
            let bot_clone = bot.clone();
            join_set.spawn(bot_clone.run());
        }
        join_set.join_all().await;
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    Message(Message),
    #[allow(dead_code)]
    Other(String),
}

#[derive(Clone, Derivative)]
#[derivative(Debug)]
pub struct Message {
    id: String,
    contents: MessageContents,
    chat: Chat,
    sender: User,

    #[derivative(Debug = "ignore")]
    bot: Option<Arc<dyn BotAPI>>,
}

impl Message {
    #[must_use]
    pub const fn new(id: String, contents: MessageContents, chat: Chat, sender: User) -> Self {
        Self {
            id,
            contents,
            chat,
            sender,

            bot: None,
        }
    }

    #[must_use]
    pub fn bot(self, bot: Arc<dyn BotAPI>) -> Self {
        Self {
            bot: Some(bot),
            ..self
        }
    }

    #[must_use]
    pub const fn get_id(&self) -> &String {
        &self.id
    }

    #[must_use]
    pub const fn get_chat(&self) -> &Chat {
        &self.chat
    }

    #[must_use]
    pub const fn get_sender(&self) -> &User {
        &self.sender
    }

    #[must_use]
    pub const fn get_contents(&self) -> &MessageContents {
        &self.contents
    }

    /// # Errors
    pub async fn reply(&self, contents: MessageContents) -> Result<String> {
        if let Some(bot) = &self.bot {
            bot.reply(contents, self.chat.clone()).await
        } else {
            anyhow::bail!("original message `{self:?}` has no bot pointer");
        }
    }
}

#[derive(Clone, Debug)]
pub struct MessageContents(Vec<MessageContent>);

impl MessageContents {
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    #[must_use]
    pub fn text<D>(self, s: D) -> Self
    where
        D: Display,
    {
        let mut contents = self.0;
        contents.push(MessageContent::Text(s.to_string()));
        Self(contents)
    }

    #[must_use]
    pub fn at(self, user: User) -> Self {
        let mut contents = self.0;
        contents.push(MessageContent::At(user));
        Self(contents)
    }
}

impl Default for MessageContents {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for MessageContents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for content in &self.0 {
            write!(f, "{content}")?;
        }

        Ok(())
    }
}

impl IntoIterator for MessageContents {
    type Item = MessageContent;

    type IntoIter = std::vec::IntoIter<MessageContent>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

#[derive(Clone, Debug)]
pub enum MessageContent {
    Text(String),
    At(User),
}

impl Display for MessageContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text(text) => write!(f, "{text}"),
            Self::At(user) => write!(f, "@{} ", user.get_nickname()),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Chat {
    Private(User),
    Group(Group),
}

impl Chat {
    #[must_use]
    pub const fn from_raw(chat_type: i32, chat_id: String) -> Self {
        match chat_type {
            1 => Self::Group(Group::new(chat_id)),
            _ => Self::Private(User::new(chat_id)),
        }
    }

    #[must_use]
    pub const fn get_id(&self) -> &String {
        match self {
            Self::Private(user) => user.get_id(),
            Self::Group(group) => group.get_id(),
        }
    }

    #[must_use]
    pub const fn type_as_i32(&self) -> i32 {
        match self {
            Self::Private(_) => 0,
            Self::Group(_) => 1,
        }
    }
}

#[derive(Clone, Debug)]
pub struct User {
    pub id: String,
    pub nickname: Option<String>,
}

impl User {
    #[must_use]
    pub const fn new(id: String) -> Self {
        Self { id, nickname: None }
    }

    #[must_use]
    pub fn nickname(self, nickname: String) -> Self {
        Self {
            nickname: Some(nickname),
            ..self
        }
    }

    #[must_use]
    pub const fn get_id(&self) -> &String {
        &self.id
    }

    #[must_use]
    pub fn get_nickname(&self) -> &str {
        self.nickname.as_ref().map_or("", |nickname| nickname)
    }
}

#[derive(Clone, Debug)]
pub struct Group {
    pub id: String,
}

impl Group {
    #[must_use]
    pub const fn new(id: String) -> Self {
        Self { id }
    }

    #[must_use]
    pub const fn get_id(&self) -> &String {
        &self.id
    }
}
