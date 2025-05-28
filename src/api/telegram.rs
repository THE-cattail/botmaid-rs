use std::fmt::Debug;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::sync::mpsc::{Receiver, Sender};
use url::Url;

use super::BotAPI;

pub struct Telegram {
    api_url: Url,

    #[allow(dead_code)]
    event_tx: Sender<crate::Event>,
    event_rx: Arc<Mutex<Receiver<crate::Event>>>,
}

impl Telegram {
    /// # Errors
    pub fn new(token: &str) -> Result<Arc<Self>> {
        let (event_tx, event_rx) = tokio::sync::mpsc::channel::<crate::Event>(1);

        let arc = Arc::new(Self {
            api_url: {
                let s = format!("https://api.telegram.org/bot{token}/");
                Url::parse(&s).with_context(|| format!("failed to parse api url `{s}`"))?
            },

            event_tx,
            event_rx: Arc::new(Mutex::new(event_rx)),
        });

        Ok(arc)
    }

    async fn handle_update(self: &Arc<Self>, update: Update) -> Result<()> {
        if let Some(message) = update.message {
            if let Some(text) = message.text {
                self.event_tx
                    .send(crate::Event::Message(
                        crate::Message::new(
                            message.message_id.to_string(),
                            crate::MessageContents::new().text(text),
                            if let Some(chat) = message.chat {
                                if chat.r#type == "private" {
                                    crate::Chat::Private(crate::User::new(chat.id.to_string()))
                                } else {
                                    crate::Chat::Group(crate::Group::new(chat.id.to_string()))
                                }
                            } else {
                                crate::Chat::Private(crate::User::new(String::new()))
                            },
                            if let Some(from) = message.from {
                                crate::User::new(from.id.to_string()).nickname(format!(
                                    "{}{}",
                                    from.first_name,
                                    from.last_name.map_or_else(String::new, |last_name| format!(
                                        " {last_name}"
                                    ))
                                ))
                            } else {
                                crate::User::new(String::new())
                            },
                        )
                        .bot(self.clone()),
                    ))
                    .await?;
            }
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

        if let Some(result) = resp.result {
            Ok(result)
        } else {
            if !resp.ok {
                anyhow::bail!(
                    "telegram api `{url_str}({method_str})` returns failed, req: `{req_debug}`, retcode: `{:?}`, error: `{:?}`",
                    resp.err_code,
                    resp.description,
                );
            }

            anyhow::bail!("telegram api `{url_str}({method_str})` returns empty data");
        }
    }
}

#[async_trait::async_trait]
impl BotAPI for Telegram {
    async fn run(self: Arc<Self>) {
        loop {
            let mut offset = 0;

            let resp: Result<Vec<Update>> = self
                .call_api(
                    "getUpdates",
                    Method::GET,
                    Some(GetUpdatesReq {
                        offset: -1,
                        timeout: 1,
                    }),
                )
                .await;
            match resp {
                Ok(updates) => {
                    for update in updates {
                        if update.update_id > offset {
                            offset = update.update_id;
                        }
                    }
                },
                Err(err) => {
                    tracing::error!("{err:?}");
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    continue;
                },
            }

            loop {
                let resp: Result<Vec<Update>> = self
                    .call_api(
                        "getUpdates",
                        Method::GET,
                        Some(GetUpdatesReq {
                            offset: offset + 1,
                            timeout: 60,
                        }),
                    )
                    .await;
                match resp {
                    Ok(updates) => {
                        for update in updates {
                            if update.update_id > offset {
                                offset = update.update_id;
                            }

                            let self_clone = self.clone();
                            tokio::spawn(async move {
                                if let Err(err) = self_clone.handle_update(update).await {
                                    tracing::error!("{err:?}");
                                }
                            });
                        }
                    },
                    Err(err) => {
                        tracing::error!("{err:?}");
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    },
                }
            }
        }
    }

    async fn next_event(&self) -> Option<crate::Event> {
        let mut events = self.event_rx.lock().await;
        events.recv().await
    }

    async fn send_msg_inner(
        &self,
        contents: crate::MessageContents,
        chat: crate::Chat,
    ) -> Result<String> {
        let mut raw = String::new();
        for content in contents {
            match content {
                crate::MessageContent::Text(text) => raw = format!("{raw}{text}"),
                crate::MessageContent::At(user) => {
                    raw = format!(
                        "{raw}{} ",
                        if let Some(nickname) = user.nickname {
                            nickname
                        } else {
                            user.id
                        }
                    );
                },
            }
        }

        let req = match chat {
            crate::Chat::Private(user) => SendMessageReq {
                chat_id: user.id.parse()?,
                text: raw,
            },
            crate::Chat::Group(group) => SendMessageReq {
                chat_id: group.id.parse()?,
                text: raw,
            },
        };

        let resp: Message = self
            .call_api("sendMessage", reqwest::Method::POST, Some(req))
            .await?;

        Ok(resp.message_id.to_string())
    }
}

#[derive(Debug, Deserialize)]
struct Resp<T> {
    ok: bool,
    result: Option<T>,
    err_code: Option<i16>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RespStatus {
    Ok,
    Async,
    Failed,
}

#[derive(Debug, Serialize)]
struct GetUpdatesReq {
    offset: i64,
    timeout: u64,
}

#[derive(Debug, Deserialize)]
struct Update {
    update_id: i64,
    message: Option<Message>,
}

#[derive(Debug, Deserialize)]
struct User {
    id: i64,
    first_name: String,
    last_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Chat {
    id: i64,
    r#type: String,
}

#[derive(Debug, Deserialize)]
struct Message {
    #[allow(clippy::struct_field_names)]
    message_id: i64,
    from: Option<User>,
    chat: Option<Chat>,
    text: Option<String>,
}

#[derive(Debug, Serialize)]
struct SendMessageReq {
    chat_id: i64,
    text: String,
}
