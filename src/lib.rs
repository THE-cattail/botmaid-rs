use std::fmt::{Debug, Display};
use std::ops::Deref;
use std::sync::Arc;

use anyhow::Result;
use api::BotAPI;
use derivative::Derivative;
use tokio::task::JoinSet;

pub mod api;

#[async_trait::async_trait]
pub trait BotInstance<C>: Send + Sync + 'static
where
    C: Clone + Debug + Send + Sync + 'static,
{
    async fn handle_msg(self: &Arc<Self>, msg: Message<C>) -> Result<()>;
    async fn run_jobs(self: &Arc<Self>, apis: &Arc<Vec<Arc<dyn BotAPI<C>>>>) -> Result<()>;
}

pub struct BotMaidBuilder<I, C>
where
    I: BotInstance<C>,
    C: Clone + Debug + Send + Sync + 'static,
{
    apis: Vec<Arc<dyn BotAPI<C>>>,
    instance: I,
}

impl<I, C> BotMaidBuilder<I, C>
where
    I: BotInstance<C>,
    C: Clone + Debug + Send + Sync + 'static,
{
    pub fn add_api<A>(&mut self, api: A) -> Arc<A>
    where
        A: BotAPI<C> + 'static,
    {
        let api = Arc::new(api);
        self.apis.push(api.clone());
        api
    }

    pub async fn run(self) {
        BotMaid {
            apis: Arc::new(self.apis),
            instance: Arc::new(self.instance),
        }
        .run()
        .await;
    }
}

pub struct BotMaid<I, C>
where
    I: BotInstance<C>,
    C: Clone + Debug + Send + Sync + 'static,
{
    apis: Arc<Vec<Arc<dyn BotAPI<C>>>>,
    instance: Arc<I>,
}

impl<I, C> BotMaid<I, C>
where
    I: BotInstance<C>,
    C: Clone + Debug + Send + Sync + 'static,
{
    #[allow(clippy::new_ret_no_self)]
    #[must_use]
    pub fn new(instance: I) -> BotMaidBuilder<I, C> {
        BotMaidBuilder {
            apis: Vec::new(),
            instance,
        }
    }

    /// # Errors
    pub async fn run(self) {
        let self_arc = Arc::new(self);

        let mut join_set = JoinSet::new();
        for api in self_arc.apis.iter() {
            let api_clone = api.clone();
            join_set.spawn(async move {
                api_clone.run().await;
            });

            let api_clone = api.clone();
            let self_clone = self_arc.clone();
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

        let self_clone = self_arc.clone();
        join_set.spawn(async move {
            if let Err(err) = self_clone.instance.run_jobs(&self_clone.apis).await {
                tracing::error!("{err:?}");
            }
        });

        join_set.join_all().await;
    }

    async fn handle_event(self: Arc<Self>, event: Event<C>) -> Result<()> {
        tracing::info!("handling event: {event:?}");

        match event {
            Event::Message(msg) => self.instance.handle_msg(msg).await,
            Event::Other(_) => Ok(()),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Event<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    Message(Message<C>),
    #[allow(dead_code)]
    Other(String),
}

#[derive(Clone, Derivative)]
#[derivative(Debug)]
pub struct Message<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    id: String,
    contents: MessageContents,
    chat: Chat<C>,
    sender: User,
}

impl<C> Message<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    #[must_use]
    pub const fn new(id: String, contents: MessageContents, chat: Chat<C>, sender: User) -> Self {
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
    pub const fn get_chat(&self) -> &Chat<C> {
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

    #[must_use]
    pub fn get_api(&self) -> &Arc<dyn BotAPI<C>> {
        &self.chat.api
    }

    #[must_use]
    pub fn get_api_context(&self) -> &C {
        self.chat.api.get_context()
    }

    #[must_use]
    pub fn be_at(&self) -> bool {
        for content in &self.contents {
            if let MessageContent::At(user) = content {
                if user.get_id() == &self.get_api().get_self_user().id {
                    return true;
                }
            }
        }

        false
    }

    /// # Errors
    pub async fn reply(&self, contents: MessageContents) -> Result<String> {
        self.chat.api.reply(contents, self.chat.clone()).await
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

impl Deref for MessageContents {
    type Target = Vec<MessageContent>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl IntoIterator for MessageContents {
    type Item = MessageContent;

    type IntoIter = std::vec::IntoIter<MessageContent>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'a> IntoIterator for &'a MessageContents {
    type Item = &'a MessageContent;

    type IntoIter = std::slice::Iter<'a, MessageContent>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
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
pub struct Chat<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    info: ChatInfo,

    #[derivative(Debug = "ignore")]
    api: Arc<dyn BotAPI<C>>,
}

impl<C> Chat<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    #[must_use]
    const fn private(api: Arc<dyn BotAPI<C>>, user: User) -> Self {
        Self {
            info: ChatInfo::Private(user),
            api,
        }
    }

    #[must_use]
    const fn group(api: Arc<dyn BotAPI<C>>, group: Group) -> Self {
        Self {
            info: ChatInfo::Group(group),
            api,
        }
    }

    #[must_use]
    pub fn spawn_private(&self, user: User) -> Self {
        Self::private(self.api.clone(), user)
    }

    #[must_use]
    pub fn spawn_group(&self, group: Group) -> Self {
        Self::group(self.api.clone(), group)
    }

    #[must_use]
    pub const fn from_raw(api: Arc<dyn BotAPI<C>>, chat_type: i32, chat_id: String) -> Self {
        Self {
            info: match chat_type {
                1 => ChatInfo::Group(Group::new(chat_id)),
                _ => ChatInfo::Private(User::new(chat_id)),
            },
            api,
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
    pub const fn get_api(&self) -> &Arc<dyn BotAPI<C>> {
        &self.api
    }

    #[must_use]
    pub fn get_api_context(&self) -> &C {
        self.api.get_context()
    }

    #[must_use]
    pub const fn type_as_i32(&self) -> i32 {
        match &self.info {
            ChatInfo::Private(_) => 0,
            ChatInfo::Group(_) => 1,
        }
    }

    /// # Errors
    pub async fn send_msg(&self, contents: MessageContents) -> Result<String> {
        self.api.send_msg(contents, self.clone()).await
    }
}

#[derive(Clone, Debug)]
pub enum ChatInfo {
    Private(User),
    Group(Group),
}

#[derive(Clone, Debug)]
pub struct User {
    id: String,
    nickname: Option<String>,
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

    /// # Errors
    pub async fn is_chat_admin<C>(&self, chat: &Chat<C>) -> Result<bool>
    where
        C: Clone + Debug + Send + Sync + 'static,
    {
        Ok(match &chat.get_info() {
            ChatInfo::Private(_) => true,
            ChatInfo::Group(group) => chat.get_api().is_group_admin(self, group).await?,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Group {
    id: String,
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
