use std::fmt::Debug;
use std::sync::Arc;

use anyhow::Result;

use crate::{Chat, Event, Group, Message, MessageContents, User};

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

        self.send_msg_inner(contents, chat, None).await
    }
    async fn reply_to_msg(
        &self,
        contents: MessageContents,
        reply_to_message: &Message<C>,
    ) -> Result<String> {
        tracing::info!("replying to message [{reply_to_message:?}]: {contents}");

        self.send_msg_inner(
            contents,
            reply_to_message.get_chat().clone(),
            Some(reply_to_message),
        )
        .await
    }
    async fn send_msg_inner(
        &self,
        contents: MessageContents,
        chat: Chat<C>,
        reply_to_message: Option<&Message<C>>,
    ) -> Result<String>;

    async fn is_group_admin(&self, user: &User, group: &Group) -> Result<bool>;
}
