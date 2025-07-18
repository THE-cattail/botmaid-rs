use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Error, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::sync::mpsc::{Receiver, Sender};
use url::Url;

use crate::BotAPI;

pub struct OneBot11<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    event_url: Url,
    api_url: Url,

    event_tx: Sender<crate::Event<C>>,
    event_rx: Arc<Mutex<Receiver<crate::Event<C>>>>,

    self_user: crate::User,

    context: C,
}

impl<C> OneBot11<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    /// # Errors
    pub async fn new(host: &str, ws_port: u16, http_port: u16, context: C) -> Result<Self>
    where
        C: Clone + Debug + Send + Sync + 'static,
    {
        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<crate::Event<C>>(1);

        let api_url = {
            let s = format!("http://{host}:{http_port}/");
            Url::parse(&s).with_context(|| format!("failed to parse api url `{s}`"))?
        };

        let resp: GetLoginInfoData = call_api(
            api_url.join("get_login_info")?,
            reqwest::Method::GET,
            None::<()>,
        )
        .await?;

        Ok(Self {
            event_url: {
                let s = format!("ws://{host}:{ws_port}/event");
                Url::parse(&s).with_context(|| format!("failed to parse event url `{s}`"))?
            },
            api_url,

            event_tx,
            event_rx: Arc::new(Mutex::new(event_rx)),

            self_user: crate::User::new(resp.user_id.to_string()).nickname(resp.nickname),

            context,
        })
    }

    async fn handle_ws_msg(
        self: &Arc<Self>,
        msg: tungstenite::Result<tungstenite::Message>,
    ) -> Result<()> {
        let msg_debug = format!("{msg:?}");

        if let tungstenite::Message::Text(text) = msg? {
            let event: Event = serde_json::from_str(&text)
                .with_context(|| format!("failed to decode json from `{text}`"))?;

            let Event::Message {
                message_id,
                message_type,
                group_id,
                user_id,
                message,
                sender,
            } = event
            else {
                return Ok(());
            };

            let mut contents = crate::MessageContents::new();

            for msg in message {
                match msg {
                    MessageSegment::Text { text } => contents = contents.text(text),
                    MessageSegment::At { qq } => {
                        contents = contents.at(crate::User::new(qq));
                    },
                    _ => (),
                }
            }

            let sender = crate::User::new(user_id.to_string()).nickname(sender.nickname);

            self.event_tx
                .send(crate::Event::Message(crate::Message::new(
                    message_id.to_string(),
                    contents,
                    match message_type {
                        MessageType::Private => crate::Chat::private(self.clone(), sender.clone()),
                        MessageType::Group => crate::Chat::group(
                            self.clone(),
                            crate::Group::new(group_id.context("no group id")?.to_string()),
                        ),
                    },
                    sender,
                )))
                .await?;
        } else {
            anyhow::bail!("`{msg_debug} is not a text");
        }

        Ok(())
    }

    async fn call_api<R, D>(
        &self,
        api: &'static str,
        method: reqwest::Method,
        req: Option<R>,
    ) -> Result<D>
    where
        R: Serialize + Debug + Send,
        D: for<'de> Deserialize<'de> + Debug,
    {
        let url = self
            .api_url
            .join(api)
            .with_context(|| format!("failed to join `{}` and {api}", self.api_url))?;

        call_api(url, method, req).await
    }
}

#[async_trait::async_trait]
impl<C> BotAPI<C> for OneBot11<C>
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
        loop {
            let (mut ws_stream, _) = match tokio_tungstenite::connect_async(self.event_url.as_str())
                .await
                .with_context(|| format!("failed to connect `{}`", self.event_url))
            {
                Ok(r) => r,
                Err(err) => {
                    tracing::error!("{err:?}");
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    continue;
                },
            };

            while let Some(msg) = ws_stream.next().await {
                let self_clone = self.clone();
                tokio::spawn(async move {
                    if let Err(err) = self_clone.handle_ws_msg(msg).await {
                        tracing::error!("{err:?}");
                    }
                });
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
        chat: crate::Chat<C>,
        reply_to_msg: Option<&crate::Message<C>>,
    ) -> Result<String> {
        let mut message = Vec::new();

        if let Some(reply_to_msg) = reply_to_msg {
            message.push(MessageSegment::Reply {
                id: reply_to_msg.id.clone(),
            });
        }

        for content in contents {
            match content {
                crate::MessageContent::Text(text) => message.push(MessageSegment::Text { text }),
                crate::MessageContent::At(user) => {
                    if chat.is_private() {
                        message.push(MessageSegment::Text {
                            text: user.get_nickname().to_string(),
                        });
                    } else {
                        message.push(MessageSegment::At { qq: user.id });
                    }

                    message.push(MessageSegment::Text {
                        text: " ".to_string(),
                    });
                },
            }
        }

        let req = match chat.get_info() {
            crate::ChatInfo::Private(user) => SendMsgReq::Private {
                user_id: user.id.parse()?,
                message,
            },
            crate::ChatInfo::Group(group) => SendMsgReq::Group {
                group_id: group.id.parse()?,
                message,
            },
        };

        let resp: SendMsgData = self
            .call_api("send_msg", reqwest::Method::POST, Some(req))
            .await?;

        Ok(resp.message_id.to_string())
    }

    async fn is_group_admin(&self, user: &crate::User, group: &crate::Group) -> Result<bool> {
        let resp: GetGroupMemberInfoData = self
            .call_api(
                "get_group_member_info",
                reqwest::Method::POST,
                Some(GetGroupMemberInfoReq {
                    group_id: group.id.parse()?,
                    user_id: user.id.parse()?,
                }),
            )
            .await?;

        Ok(resp.role == GroupMemberInfoRole::Owner || resp.role == GroupMemberInfoRole::Admin)
    }
}

async fn call_api<R, D>(url: Url, method: reqwest::Method, req: Option<R>) -> Result<D>
where
    R: Serialize + Debug + Send,
    D: for<'de> Deserialize<'de> + Debug,
{
    let url_str = format!("{url}");
    let method_str = format!("{method}");
    let req_debug = format!("{req:?}");

    let resp: Resp<D> = food_http_rs::call_api(url, method, req)
        .await
        .with_context(|| {
            format!("failed to call api `{url_str}({method_str})`, req: `{req_debug}`")
        })?;

    if let Some(data) = resp.data {
        Ok(data)
    } else {
        if matches!(resp.status, RespStatus::Failed) {
            anyhow::bail!(
                "onebot 11 api `{url_str}({method_str})` returns failed, req: `{req_debug}`, retcode: `{}`, error: `{}`",
                resp.retcode,
                resp.message
            );
        }

        anyhow::bail!("onebot 11 api `{url_str}({method_str})` returns empty data");
    }
}

#[derive(Debug, Deserialize)]
struct Resp<T> {
    status: RespStatus,
    retcode: u16,
    data: Option<T>,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RespStatus {
    Ok,
    Async,
    Failed,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
enum MessageSegment {
    Text { text: String },
    Face { id: String },
    Image { file: String },
    Record { file: String },
    Video { file: String },
    At { qq: String },
    Rps {},
    Dice {},
    Shake {},
    Poke { r#type: String, id: String },
    Anonymous {},
    Share { url: String, title: String },
    Contact { r#type: String, id: String },
    Location { lat: String, lon: String },
    Music { r#type: String },
    Reply { id: String },
    Forward { id: String },
    Node {},
    Xml { data: String },
    Json { data: String },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "post_type", rename_all = "snake_case")]
enum Event {
    Message {
        message_id: i64,
        message_type: MessageType,
        group_id: Option<i64>,
        user_id: i64,
        message: Vec<MessageSegment>,
        sender: MessageSender,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MessageType {
    Private,
    Group,
}

#[derive(Debug, Deserialize)]
struct MessageSender {
    nickname: String,
}

impl TryFrom<&str> for Event {
    type Error = Error;

    fn try_from(value: &str) -> Result<Self> {
        serde_json::from_str(value).with_context(|| format!("failed to deserialize json `{value}`"))
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "message_type", rename_all = "snake_case")]
enum SendMsgReq {
    Private {
        user_id: i64,
        message: Vec<MessageSegment>,
    },
    Group {
        group_id: i64,
        message: Vec<MessageSegment>,
    },
}

#[derive(Debug, Deserialize)]
struct SendMsgData {
    message_id: i64,
}

#[derive(Debug, Serialize)]
struct GetGroupMemberInfoReq {
    group_id: i64,
    user_id: i64,
}

#[derive(Debug, Deserialize)]
struct GetGroupMemberInfoData {
    role: GroupMemberInfoRole,
}

#[derive(Debug, PartialEq, Deserialize)]
enum GroupMemberInfoRole {
    Owner,
    Admin,
    Member,
}

#[derive(Debug, Deserialize)]
struct GetLoginInfoData {
    user_id: i64,
    nickname: String,
}
