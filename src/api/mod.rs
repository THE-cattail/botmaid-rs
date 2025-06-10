use std::fmt::Debug;
use std::sync::Arc;

use anyhow::Result;

use crate::{Chat, Event, Group, MessageContents, User};

pub mod cli;
pub mod mock;
pub mod onebot_11;
pub mod telegram;

#[async_trait::async_trait]
pub trait BotAPI<C>: Send + Sync + 'static
where
    C: Clone + Debug + Send + Sync + 'static,
{
    fn get_context(&self) -> &C;
    fn get_self_user(&self) -> &User;

    async fn run(self: Arc<Self>);

    async fn next_event(&self) -> Option<Event<C>>;

    async fn send_msg(&self, contents: MessageContents, chat: Chat<C>) -> Result<String> {
        tracing::info!("sending message to [{chat:?}]: {contents}");

        self.send_msg_inner(contents, chat).await
    }
    async fn send_msg_inner(&self, contents: MessageContents, chat: Chat<C>) -> Result<String>;
    async fn reply(&self, contents: MessageContents, chat: Chat<C>) -> Result<String> {
        self.send_msg(contents, chat).await
    }

    async fn is_group_admin(&self, user: &User, group: &Group) -> Result<bool>;
}
