use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};

pub struct TelegramClient {
    client: Client,
    base_url: String,
    pub token: String,
}

// ── Types ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<Message>,
    pub callback_query: Option<CallbackQuery>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    pub message_id: i64,
    pub from: Option<User>,
    pub chat: Chat,
    pub text: Option<String>,
    pub caption: Option<String>,
    pub photo: Option<Vec<PhotoSize>>,
    pub document: Option<Document>,
    pub voice: Option<Voice>,
    pub video: Option<Video>,
    pub sticker: Option<Sticker>,
    pub reply_to_message: Option<Box<Message>>,
    pub message_thread_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct User {
    pub id: i64,
    pub is_bot: Option<bool>,
    pub first_name: String,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type")]
    pub chat_type: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct PhotoSize {
    pub file_id: String,
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Document {
    pub file_id: String,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Voice {
    pub file_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Video {
    pub file_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Sticker {
    pub file_id: String,
    pub emoji: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct CallbackQuery {
    pub id: String,
    pub from: User,
    pub message: Option<Message>,
    pub data: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct TgFile {
    pub file_id: String,
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TgResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

// ── Send options ───────────────────────────────────────────────────────────

#[derive(Default)]
pub struct SendOpts {
    pub reply_to_message_id: Option<i64>,
    pub message_thread_id: Option<i64>,
    pub parse_mode: Option<String>,
    pub reply_markup: Option<Value>,
}

// ── Client ─────────────────────────────────────────────────────────────────

impl TelegramClient {
    pub fn new(token: &str) -> Self {
        TelegramClient {
            client: Client::new(),
            base_url: format!("https://api.telegram.org/bot{}", token),
            token: token.to_string(),
        }
    }

    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        body: Value,
    ) -> Result<T, String> {
        let url = format!("{}/{}", self.base_url, method);
        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("{}: {}", method, e))?;

        let tg_resp: TgResponse<T> = resp
            .json()
            .await
            .map_err(|e| format!("{} parse: {}", method, e))?;

        if tg_resp.ok {
            tg_resp
                .result
                .ok_or_else(|| format!("{}: no result", method))
        } else {
            Err(format!(
                "{}: {}",
                method,
                tg_resp.description.unwrap_or_default()
            ))
        }
    }

    pub async fn get_me(&self) -> Result<User, String> {
        self.call("getMe", json!({})).await
    }

    pub async fn get_updates(
        &self,
        offset: i64,
        timeout: u32,
    ) -> Result<Vec<Update>, String> {
        self.call(
            "getUpdates",
            json!({ "offset": offset, "timeout": timeout, "allowed_updates": ["message", "callback_query"] }),
        )
        .await
    }

    pub async fn send_message(
        &self,
        chat_id: &str,
        text: &str,
        opts: &SendOpts,
    ) -> Result<Message, String> {
        let mut body = json!({
            "chat_id": chat_id,
            "text": text,
        });
        if let Some(id) = opts.reply_to_message_id {
            body["reply_to_message_id"] = json!(id);
        }
        if let Some(id) = opts.message_thread_id {
            body["message_thread_id"] = json!(id);
        }
        if let Some(ref mode) = opts.parse_mode {
            body["parse_mode"] = json!(mode);
        }
        if let Some(ref markup) = opts.reply_markup {
            body["reply_markup"] = markup.clone();
        }
        body["link_preview_options"] = json!({"is_disabled": true});
        self.call("sendMessage", body).await
    }

    pub async fn edit_message_text(
        &self,
        chat_id: &str,
        message_id: i64,
        text: &str,
    ) -> Result<(), String> {
        let _: Value = self
            .call(
                "editMessageText",
                json!({
                    "chat_id": chat_id,
                    "message_id": message_id,
                    "text": text,
                    "link_preview_options": {"is_disabled": true},
                }),
            )
            .await?;
        Ok(())
    }

    pub async fn edit_message_text_markup(
        &self,
        chat_id: &str,
        message_id: i64,
        text: &str,
        reply_markup: &Value,
    ) -> Result<(), String> {
        let _: Value = self
            .call(
                "editMessageText",
                json!({
                    "chat_id": chat_id,
                    "message_id": message_id,
                    "text": text,
                    "reply_markup": reply_markup,
                    "link_preview_options": {"is_disabled": true},
                }),
            )
            .await?;
        Ok(())
    }

    pub async fn edit_message_reply_markup(
        &self,
        chat_id: &str,
        message_id: i64,
        reply_markup: &Value,
    ) -> Result<(), String> {
        let _: Value = self
            .call(
                "editMessageReplyMarkup",
                json!({
                    "chat_id": chat_id,
                    "message_id": message_id,
                    "reply_markup": reply_markup,
                }),
            )
            .await?;
        Ok(())
    }

    pub async fn delete_message(&self, chat_id: &str, message_id: i64) -> Result<(), String> {
        let _: Value = self
            .call(
                "deleteMessage",
                json!({
                    "chat_id": chat_id,
                    "message_id": message_id,
                }),
            )
            .await?;
        Ok(())
    }


    pub async fn answer_callback_query(
        &self,
        callback_query_id: &str,
        text: Option<&str>,
    ) -> Result<(), String> {
        let mut body = json!({ "callback_query_id": callback_query_id });
        if let Some(t) = text {
            body["text"] = json!(t);
        }
        let _: Value = self.call("answerCallbackQuery", body).await?;
        Ok(())
    }

    pub async fn get_file(&self, file_id: &str) -> Result<TgFile, String> {
        self.call("getFile", json!({ "file_id": file_id })).await
    }

    pub async fn set_my_commands(&self, commands: &[Value]) -> Result<(), String> {
        let _: Value = self
            .call("setMyCommands", json!({ "commands": commands }))
            .await?;
        Ok(())
    }

    pub fn file_url(&self, file_path: &str) -> String {
        format!(
            "https://api.telegram.org/file/bot{}/{}",
            self.token, file_path
        )
    }

    pub async fn download_file_bytes(&self, file_path: &str) -> Result<Vec<u8>, String> {
        let url = self.file_url(file_path);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("download failed: {}", resp.status()));
        }
        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| e.to_string())
    }
}
