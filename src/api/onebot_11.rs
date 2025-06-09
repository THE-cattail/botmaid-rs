use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Error, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::sync::mpsc::{Receiver, Sender};
use url::Url;

use super::BotAPI;

pub struct OneBot11<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    event_url: Url,
    api_url: Url,

    event_tx: Sender<crate::Event<C>>,
    event_rx: Arc<Mutex<Receiver<crate::Event<C>>>>,

    context: C,
}

impl<C> OneBot11<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    /// # Errors
    pub fn new(host: &str, ws_port: u16, http_port: u16, context: C) -> Result<Self>
    where
        C: Clone + Debug + Send + Sync + 'static,
    {
        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<crate::Event<C>>(1);

        Ok(Self {
            event_url: {
                let s = format!("ws://{host}:{ws_port}/event");
                Url::parse(&s).with_context(|| format!("failed to parse event url `{s}`"))?
            },
            api_url: {
                let s = format!("http://{host}:{http_port}/");
                Url::parse(&s).with_context(|| format!("failed to parse api url `{s}`"))?
            },

            event_tx,
            event_rx: Arc::new(Mutex::new(event_rx)),

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
                .with_context(|| "failed to decode json from `{text}`")?;

            let event = match event {
                Event::Message {
                    message_id,
                    message_type,
                    group_id,
                    user_id,
                    raw_message,
                    sender,
                } => {
                    let sender = crate::User::new(user_id.to_string()).nickname(sender.nickname);
                    crate::Event::Message(crate::Message::new(
                        message_id.to_string(),
                        crate::MessageContents::new().text(raw_message),
                        match message_type {
                            MessageType::Private => {
                                crate::Chat::private(self.clone(), sender.clone())
                            },
                            MessageType::Group => crate::Chat::group(
                                self.clone(),
                                crate::Group::new(group_id.context("no group id")?.to_string()),
                            ),
                        },
                        sender,
                    ))
                },
                Event::Notice | Event::Request | Event::Meta => {
                    crate::Event::Other(format!("{event:?}"))
                },
            };

            self.event_tx.send(event).await?;
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
}

#[async_trait::async_trait]
impl<C> BotAPI<C> for OneBot11<C>
where
    C: Clone + Debug + Send + Sync + 'static,
{
    fn get_context(&self) -> &C {
        &self.context
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
    ) -> Result<String> {
        let mut raw = String::new();
        for content in contents {
            match content {
                crate::MessageContent::Text(text) => raw = format!("{raw}{text}"),
                crate::MessageContent::At(user) => {
                    raw = format!("{raw}[CQ:at,qq={}] ", user.id);
                },
            }
        }

        let req = match chat.get_info() {
            crate::ChatInfo::Private(user) => SendMsgReq::Private {
                user_id: user.id.parse()?,
                message: raw,
            },
            crate::ChatInfo::Group(group) => SendMsgReq::Group {
                group_id: group.id.parse()?,
                message: raw,
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

#[derive(Debug, Deserialize)]
#[serde(tag = "post_type", rename_all = "snake_case")]
enum Event {
    Message {
        message_id: i64,
        message_type: MessageType,
        group_id: Option<i64>,
        user_id: i64,
        raw_message: String,
        sender: MessageSender,
    },
    Notice,
    Request,
    #[serde(rename = "meta_event")]
    Meta,
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
    Private { user_id: i64, message: String },
    Group { group_id: i64, message: String },
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
