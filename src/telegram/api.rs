use std::path::Path;

use anyhow::{Context, Result, bail};
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

#[derive(Debug, Clone)]
pub struct TelegramClient {
    http: reqwest::Client,
    base_url: String,
}

#[derive(Debug, Deserialize)]
pub struct TelegramResponse<T> {
    pub ok: bool,
    pub result: Option<T>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramUser {
    pub id: i64,
    pub first_name: String,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramChat {
    pub id: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramMessage {
    pub message_id: i64,
    pub chat: TelegramChat,
    pub from: Option<TelegramUser>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CallbackQuery {
    pub id: String,
    pub from: TelegramUser,
    pub data: Option<String>,
    pub message: Option<TelegramMessage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<TelegramMessage>,
    pub callback_query: Option<CallbackQuery>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InlineKeyboardMarkup {
    pub inline_keyboard: Vec<Vec<InlineKeyboardButton>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BotCommand {
    pub command: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InlineKeyboardButton {
    pub text: String,
    pub callback_data: String,
}

#[derive(Debug, Serialize)]
struct GetUpdatesRequest {
    offset: i64,
    timeout: u64,
    allowed_updates: Vec<&'static str>,
}

#[derive(Debug, Serialize)]
struct SendMessageRequest<'a> {
    chat_id: i64,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_markup: Option<&'a InlineKeyboardMarkup>,
}

#[derive(Debug, Serialize)]
struct EditMessageTextRequest<'a> {
    chat_id: i64,
    message_id: i64,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_markup: Option<&'a InlineKeyboardMarkup>,
}

#[derive(Debug, Serialize)]
struct AnswerCallbackQueryRequest<'a> {
    callback_query_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct SetMyCommandsRequest<'a> {
    commands: &'a [BotCommand],
}

impl TelegramClient {
    pub fn new(bot_token: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: format!("{TELEGRAM_API_BASE}/bot{bot_token}"),
        }
    }

    pub async fn get_me(&self) -> Result<TelegramUser> {
        self.post_json::<_, TelegramUser>("getMe", &serde_json::json!({}))
            .await
    }

    pub async fn get_updates(&self, offset: i64, timeout: u64) -> Result<Vec<Update>> {
        let req = GetUpdatesRequest {
            offset,
            timeout,
            allowed_updates: vec!["message", "callback_query"],
        };
        self.post_json("getUpdates", &req).await
    }

    pub async fn set_my_commands(&self, commands: &[BotCommand]) -> Result<()> {
        let req = SetMyCommandsRequest { commands };
        let _: bool = self.post_json("setMyCommands", &req).await?;
        Ok(())
    }

    pub async fn send_message(
        &self,
        chat_id: i64,
        text: &str,
        keyboard: Option<&InlineKeyboardMarkup>,
    ) -> Result<TelegramMessage> {
        let req = SendMessageRequest {
            chat_id,
            text,
            reply_markup: keyboard,
        };
        self.post_json("sendMessage", &req).await
    }

    pub async fn edit_message_text(
        &self,
        chat_id: i64,
        message_id: i64,
        text: &str,
        keyboard: Option<&InlineKeyboardMarkup>,
    ) -> Result<TelegramMessage> {
        let req = EditMessageTextRequest {
            chat_id,
            message_id,
            text,
            reply_markup: keyboard,
        };
        self.post_json("editMessageText", &req).await
    }

    pub async fn answer_callback_query(&self, query_id: &str, text: Option<&str>) -> Result<()> {
        let req = AnswerCallbackQueryRequest {
            callback_query_id: query_id,
            text,
        };
        let _: bool = self.post_json("answerCallbackQuery", &req).await?;
        Ok(())
    }

    pub async fn send_document(
        &self,
        chat_id: i64,
        file_path: &Path,
        caption: &str,
    ) -> Result<TelegramMessage> {
        let file_name = file_path
            .file_name()
            .and_then(|value| value.to_str())
            .context("document path must include a valid file name")?;
        let bytes = tokio::fs::read(file_path)
            .await
            .with_context(|| format!("failed to read document {}", file_path.display()))?;
        let document = Part::bytes(bytes).file_name(file_name.to_string());
        let form = Form::new()
            .text("chat_id", chat_id.to_string())
            .text("caption", caption.to_string())
            .part("document", document);
        self.post_multipart("sendDocument", form).await
    }

    async fn post_json<S: Serialize, T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        payload: &S,
    ) -> Result<T> {
        let response = self
            .http
            .post(format!("{}/{}", self.base_url, method))
            .json(payload)
            .send()
            .await
            .with_context(|| format!("telegram {method} request failed"))?;
        let parsed = response
            .json::<TelegramResponse<T>>()
            .await
            .with_context(|| format!("telegram {method} response parse failed"))?;
        decode_response(method, parsed)
    }

    async fn post_multipart<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        payload: Form,
    ) -> Result<T> {
        let response = self
            .http
            .post(format!("{}/{}", self.base_url, method))
            .multipart(payload)
            .send()
            .await
            .with_context(|| format!("telegram {method} request failed"))?;
        let parsed = response
            .json::<TelegramResponse<T>>()
            .await
            .with_context(|| format!("telegram {method} response parse failed"))?;
        decode_response(method, parsed)
    }
}

fn decode_response<T>(method: &str, response: TelegramResponse<T>) -> Result<T> {
    if !response.ok {
        bail!(
            "telegram {} failed: {}",
            method,
            response
                .description
                .unwrap_or_else(|| "unknown error".to_string())
        );
    }
    response
        .result
        .with_context(|| format!("telegram {method} response missing result"))
}

pub fn default_bot_commands() -> Vec<BotCommand> {
    vec![
        BotCommand {
            command: "start".into(),
            description: "Show help and available commands".into(),
        },
        BotCommand {
            command: "help".into(),
            description: "Show help and available commands".into(),
        },
        BotCommand {
            command: "status".into(),
            description: "Show current repo, thread, and runtime status".into(),
        },
        BotCommand {
            command: "abort".into(),
            description: "Abort the active Codex turn".into(),
        },
        BotCommand {
            command: "repo".into(),
            description: "Repo commands: list, use, clone, status, rescan".into(),
        },
        BotCommand {
            command: "thread".into(),
            description: "Thread commands: list, new, use, status".into(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telegram_user_deserializes_snake_case_fields() {
        let raw = serde_json::json!({
            "id": 123456,
            "is_bot": true,
            "first_name": "MyCodex",
            "username": "mycodex_bot"
        });
        let user: TelegramUser =
            serde_json::from_value(raw).expect("telegram user should deserialize");
        assert_eq!(user.id, 123456);
        assert_eq!(user.first_name, "MyCodex");
        assert_eq!(user.username.as_deref(), Some("mycodex_bot"));
    }
}
