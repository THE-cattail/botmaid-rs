use std::fmt::Display;
use std::sync::Arc;

use anyhow::Result;
use api::BotAPI;
use derivative::Derivative;
use tokio::sync::RwLock;
use tokio::task::JoinSet;

pub mod api;

#[async_trait::async_trait]
pub trait BotInstance: Send + Sync {
    async fn handle_msg(self: &Arc<Self>, msg: Message) -> Result<()>;
    async fn run_jobs(self: &Arc<Self>, apis: &[Arc<dyn BotAPI>]) -> Result<()>;
}

pub struct BotMaid<I>
where
    I: BotInstance + 'static,
{
    apis: RwLock<Vec<Arc<dyn BotAPI>>>,

    instance: Arc<I>,
}

impl<I> BotMaid<I>
where
    I: BotInstance + 'static,
{
    #[must_use]
    pub fn new(instance: I) -> Arc<Self> {
        Arc::new(Self {
            apis: RwLock::new(Vec::new()),
            instance: Arc::new(instance),
        })
    }

    /// # Errors
    pub async fn add_api<A>(self: &Arc<Self>, api: A) -> Arc<A>
    where
        A: BotAPI + 'static,
    {
        let api = Arc::new(api);
        self.apis.write().await.push(api.clone());
        api
    }

    /// # Errors
    pub async fn run(self: Arc<Self>) {
        let mut join_set = JoinSet::new();
        for api in &*self.apis.read().await {
            let api_clone = api.clone();
            join_set.spawn(async move {
                api_clone.run().await;
            });

            let api_clone = api.clone();
            let self_clone = self.clone();
            join_set.spawn(async move {
                while let Some(event) = api_clone.next_event().await {
                    let self_clone = self_clone.clone();
                    tokio::spawn(async move {
                        if let Err(err) = self_clone.handle_event(event).await {
                            tracing::error!("{err:?}");
                        }
                    });
                }
            });
        }

        let self_clone = self.clone();
        join_set.spawn(async move {
            let apis = self_clone.apis.read().await;
            if let Err(err) = self_clone.instance.run_jobs(&apis).await {
                tracing::error!("{err:?}");
            }
        });

        join_set.join_all().await;
    }

    async fn handle_event(self: Arc<Self>, event: Event) -> Result<()> {
        tracing::info!("handling event: {event:?}");

        match event {
            Event::Message(msg) => self.instance.handle_msg(msg).await,
            Event::Other(_) => Ok(()),
        }
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
}

impl Message {
    #[must_use]
    pub const fn new(id: String, contents: MessageContents, chat: Chat, sender: User) -> Self {
        Self {
            id,
            contents,
            chat,
            sender,
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
        if let Some(api) = &self.chat.get_api() {
            api.reply(contents, self.chat.clone()).await
        } else {
            anyhow::bail!("original chat `{:?}` has no bot pointer", self.chat);
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

#[derive(Clone, Derivative)]
#[derivative(Debug)]
pub struct Chat {
    info: ChatInfo,

    #[derivative(Debug = "ignore")]
    api: Option<Arc<dyn BotAPI>>,
}

impl Chat {
    #[must_use]
    pub const fn private(user: User) -> Self {
        Self {
            info: ChatInfo::Private(user),
            api: None,
        }
    }

    #[must_use]
    pub const fn group(group: Group) -> Self {
        Self {
            info: ChatInfo::Group(group),
            api: None,
        }
    }

    #[must_use]
    pub const fn from_raw(chat_type: i32, chat_id: String) -> Self {
        Self {
            info: match chat_type {
                1 => ChatInfo::Group(Group::new(chat_id)),
                _ => ChatInfo::Private(User::new(chat_id)),
            },
            api: None,
        }
    }

    #[must_use]
    pub fn api(self, api: Arc<dyn BotAPI>) -> Self {
        Self {
            api: Some(api),
            ..self
        }
    }

    #[must_use]
    pub const fn get_info(&self) -> &ChatInfo {
        &self.info
    }

    #[must_use]
    pub const fn get_id(&self) -> &String {
        match &self.info {
            ChatInfo::Private(user) => user.get_id(),
            ChatInfo::Group(group) => group.get_id(),
        }
    }

    #[must_use]
    pub const fn get_api(&self) -> &Option<Arc<dyn BotAPI>> {
        &self.api
    }

    #[must_use]
    pub const fn type_as_i32(&self) -> i32 {
        match &self.info {
            ChatInfo::Private(_) => 0,
            ChatInfo::Group(_) => 1,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ChatInfo {
    Private(User),
    Group(Group),
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
