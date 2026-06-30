use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::Duration;
use tracing::{info, warn};

use crate::sources::youtube_clients::ClientKind;

/// YouTube Live Chat handler — bootstraps watch-next continuation,
/// polls InnerTube live chat endpoint, and emits structured messages.
pub struct YouTubeLiveChat {
    http_client: reqwest::Client,
    api_key: String,
    /// Active chat sessions keyed by connection id
    active_chats: Arc<Mutex<HashMap<String, bool>>>,
}

#[derive(Debug, Clone)]
pub struct LiveChatMessage {
    pub r#type: LiveChatMessageType,
    pub id: String,
    pub timestamp: u64,
    pub author_name: String,
    pub author_id: String,
    pub author_photo: Option<String>,
    pub author_badges: Vec<String>,
    pub message: String,
    pub amount: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiveChatMessageType {
    Text,
    Paid,
    Membership,
    Gift,
}

impl LiveChatMessageType {
    fn as_str(&self) -> &str {
        match self {
            Self::Text => "text",
            Self::Paid => "paid",
            Self::Membership => "membership",
            Self::Gift => "gift",
        }
    }
}

/// Poll result returned by a live chat session
#[derive(Debug, Clone)]
pub struct LiveChatPollResult {
    pub actions: Vec<serde_json::Value>,
    pub timeout_ms: u64,
}

/// Pollable live chat connection
pub struct LiveChatConnection {
    continuation: Option<String>,
    api_key: String,
    http_client: reqwest::Client,
    context: Option<Value>,
}

impl LiveChatConnection {
    pub async fn poll(&mut self) -> Option<LiveChatPollResult> {
        let continuation = self.continuation.as_ref()?;
        let payload = json!({
            "continuation": continuation,
        });
        let mut req = self.http_client
            .post(format!("https://www.youtube.com/youtubei/v1/live_chat/get_live_chat?key={}", self.api_key))
            .json(&payload)
            .header("Content-Type", "application/json");
        if let Some(ref ctx) = self.context {
            let mut body = json!({ "context": ctx });
            if let Some(payload_obj) = payload.as_object() {
                if let Some(body_obj) = body.as_object_mut() {
                    for (k, v) in payload_obj {
                        body_obj.insert(k.clone(), v.clone());
                    }
                }
            }
            req = self.http_client
                .post(format!("https://www.youtube.com/youtubei/v1/live_chat/get_live_chat?key={}", self.api_key))
                .json(&body)
                .header("Content-Type", "application/json");
        }
        let resp = req.send().await.ok()?;
        if !resp.status().is_success() { return None; }
        let data: Value = resp.json().await.ok()?;

        let continuation_contents = match data["continuationContents"]["liveChatContinuation"].as_object() {
            Some(c) => c.clone(),
            None => return None,
        };
        let next_cont = continuation_contents.get("continuations")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c["invalidationContinuationData"]["continuation"].as_str()
                .or_else(|| c["timedContinuationData"]["continuation"].as_str()))
            .map(|s| s.to_string());
        self.continuation = next_cont;

        let timeout_ms = continuation_contents.get("continuations")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c["timedContinuationData"]["timeoutMs"].as_u64()
                .or_else(|| c["invalidationContinuationData"]["timeoutMs"].as_u64()))
            .unwrap_or(5000);

        let actions = parse_actions(continuation_contents.get("actions").and_then(|v| v.as_array()).unwrap_or(&vec![]));
        Some(LiveChatPollResult { actions, timeout_ms })
    }
}

fn parse_actions(actions: &[Value]) -> Vec<Value> {
    let mut parsed = Vec::new();
    for action in actions {
        let item = match action["addChatItemAction"]["item"].as_object() {
            Some(item) => item,
            None => continue,
        };
        let (msg_type, renderer) = if let Some(r) = item.get("liveChatTextMessageRenderer") {
            ("text", r)
        } else if let Some(r) = item.get("liveChatPaidMessageRenderer") {
            ("paid", r)
        } else if let Some(r) = item.get("liveChatMembershipItemRenderer") {
            ("membership", r)
        } else if let Some(r) = item.get("liveChatSponsorshipsGiftPurchaseAnnouncementRenderer") {
            ("gift", r)
        } else {
            continue;
        };

        let author_name: String = if let Some(name) = renderer["authorName"]["simpleText"].as_str() {
            name.to_string()
        } else if let Some(runs) = renderer["headerPrimaryText"]["runs"].as_array() {
            runs.iter().filter_map(|r| r["text"].as_str()).collect()
        } else {
            String::new()
        };

        let message: String = if let Some(runs) = renderer["message"]["runs"].as_array() {
            runs.iter().filter_map(|r| r["text"].as_str()).collect()
        } else if let Some(text) = renderer["headerSubtext"]["simpleText"].as_str() {
            text.to_string()
        } else if let Some(runs) = renderer["headerSubtext"]["runs"].as_array() {
            runs.iter().filter_map(|r| r["text"].as_str()).collect()
        } else {
            String::new()
        };

        let badges: Vec<String> = renderer["authorBadges"].as_array()
            .map(|badges| badges.iter()
                .filter_map(|b| b["liveChatAuthorBadgeRenderer"]["tooltip"].as_str())
                .map(|s| s.to_string())
                .collect())
            .unwrap_or_default();

        parsed.push(json!({
            "type": msg_type,
            "id": renderer["id"].as_str().unwrap_or(""),
            "timestamp": renderer["timestampUsec"].as_str().unwrap_or("0"),
            "author": {
                "name": author_name,
                "id": renderer["authorExternalChannelId"].as_str().unwrap_or(""),
                "photo": renderer["authorPhoto"]["thumbnails"].as_array()
                    .and_then(|t| t.last())
                    .and_then(|t| t["url"].as_str())
                    .map(|s| s.to_string()),
                "badges": badges,
            },
            "message": message,
            "amount": renderer["purchaseAmountText"]["simpleText"].as_str().map(|s| s.to_string()),
        }));
    }
    parsed
}

impl YouTubeLiveChat {
    pub fn new(api_key: String) -> Self {
        Self {
            http_client: reqwest::Client::new(),
            api_key: if api_key.is_empty() { "AIzaSyAO_FJ2SlqI87oz4cl9Sdr_LRIPvS6S8".to_string() } else { api_key },
            active_chats: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn get_live_chat(&self, video_id: &str, context: Option<Value>) -> Option<LiveChatConnection> {
        let (data, status_code) = self.fetch_next_data(video_id, &context).await;
        if status_code != 200 { return None; }

        let chat_renderer = match data["contents"]["twoColumnWatchNextResults"]["conversationBar"]["liveChatRenderer"].as_object() {
            Some(r) => r,
            None => return None,
        };
        let continuation = chat_renderer.get("continuations")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|c| c["reloadContinuationData"]["continuation"].as_str())
            .map(|s| s.to_string())?;

        let api_key = data["responseContext"]["serviceTrackingParams"]
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|p| p["serviceInfo"].as_array())
            .and_then(|info| info.first())
            .and_then(|i| i["value"].as_str())
            .unwrap_or(&self.api_key)
            .to_string();

        Some(LiveChatConnection {
            continuation: Some(continuation),
            api_key,
            http_client: self.http_client.clone(),
            context,
        })
    }

    async fn fetch_next_data(&self, video_id: &str, context: &Option<Value>) -> (Value, u16) {
        let _client = ClientKind::Web;
        let payload = json!({
            "videoId": video_id,
            "context": context.as_ref().unwrap_or(&json!({})),
        });
        let url = format!("https://www.youtube.com/youtubei/v1/next?key={}&prettyPrint=false", self.api_key);
        match self.http_client.post(&url).json(&payload).send().await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let data = resp.json::<Value>().await.unwrap_or_default();
                (data, status)
            }
            Err(_) => (Value::Null, 0),
        }
    }

    pub async fn handle_live_chat(&self, video_id: &str, context: Option<Value>) -> Option<LiveChatConnection> {
        self.get_live_chat(video_id, context).await
    }

    pub async fn handle_connection<F>(&self, video_id: &str, context: Option<Value>, send_fn: F)
    where
        F: Fn(Value) + Send + 'static,
    {
        info!(target: "YouTube-LiveChat", "Starting live chat for video: {video_id}");
        let chat = match self.get_live_chat(video_id, context).await {
            Some(c) => c,
            None => {
                warn!(target: "YouTube-LiveChat", "Could not initialize live chat for {video_id}");
                return;
            }
        };

        let chat_key = format!("{video_id}-{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis());
        self.active_chats.lock().await.insert(chat_key.clone(), true);

        let mut conn = chat;
        while self.active_chats.lock().await.contains_key(&chat_key) {
            match conn.poll().await {
                Some(result) => {
                    if result.actions.is_empty() {
                        tokio::time::sleep(Duration::from_millis(result.timeout_ms)).await;
                        continue;
                    }
                    send_fn(json!({ "op": "actions", "actions": result.actions }));
                    tokio::time::sleep(Duration::from_millis(result.timeout_ms)).await;
                }
                None => break,
            }
        }

        self.active_chats.lock().await.remove(&chat_key);
        info!(target: "YouTube-LiveChat", "Live chat ended for {video_id}");
    }

    pub fn cancel(&self) {
        // External cancel is handled by dropping the active_chats
    }
}

/// Start polling live chat. Returns a receiver for chat messages (legacy API).
pub async fn connect_live_chat(video_id: &str, api_key: &str) -> anyhow::Result<tokio::sync::mpsc::UnboundedReceiver<Value>> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let chat = YouTubeLiveChat::new(api_key.to_string());
    let conn = chat.get_live_chat(video_id, None).await
        .ok_or_else(|| anyhow::anyhow!("No live chat available"))?;

    let mut conn = conn;
    tokio::spawn(async move {
        loop {
            match conn.poll().await {
                Some(result) => {
                    for action in result.actions {
                        let _ = tx.send(action);
                    }
                    tokio::time::sleep(Duration::from_millis(result.timeout_ms)).await;
                }
                None => break,
            }
        }
    });

    Ok(rx)
}
