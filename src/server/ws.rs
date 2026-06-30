use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{error, info, trace, warn};

use crate::server::ws_ops::{handle_op, LavalinkOp};
use crate::state::SharedState;

pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    headers: axum::http::HeaderMap,
    State(state): State<SharedState>,
) -> Response {
    let user_id = headers
        .get("User-Id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("0")
        .to_string();

    let resume_session_id = headers
        .get("Session-Id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    ws.on_upgrade(move |socket| handle_socket(socket, state, user_id, resume_session_id))
}

async fn handle_socket(
    socket: WebSocket,
    state: SharedState,
    user_id: String,
    resume_session_id: Option<String>,
) {
    info!(target: "WebSocket", "Connection established from client.");

    let (mut sender, mut receiver) = socket.split();

    // Try to resume an existing session
    let (session_id, resumed) = if let Some(ref sid) = resume_session_id {
        if let Some(session) = state.sessions.get(sid) {
            if session.resuming {
                info!(target: "WebSocket", "Resuming session: {}", sid);
                (sid.clone(), true)
            } else {
                let new_id = uuid::Uuid::new_v4().to_string();
                info!(
                    target: "WebSocket",
                    "Session {} not resumable, creating new: {}",
                    sid, new_id
                );
                (new_id, false)
            }
        } else {
            let new_id = uuid::Uuid::new_v4().to_string();
            info!(target: "WebSocket", "No session to resume, creating: {}", new_id);
            (new_id, false)
        }
    } else {
        let new_id = uuid::Uuid::new_v4().to_string();
        info!(target: "WebSocket", "No Session-Id header, creating new: {}", new_id);
        (new_id, false)
    };

    // NodeLink/Lavalink Handshake
    let ready_msg = json!({
        "op": "ready",
        "resumed": resumed,
        "sessionId": session_id.clone()
    });

    if let Err(e) = sender.send(Message::Text(ready_msg.to_string())).await {
        error!(target: "WebSocket", "Failed to send ready payload: {}", e);
        return;
    }

    // Create a channel for receiving messages from PlayerWorkers or handle_op
    let (tx, mut rx) = mpsc::channel::<serde_json::Value>(100);

    // Register the sender in our global state
    {
        let mut ws_senders = state.ws_senders.write().await;
        ws_senders.insert(session_id.clone(), tx);
    }

    // Store/update the session in AppState
    if !resumed {
        state.sessions.insert(
            session_id.clone(),
            crate::state::Session {
                id: session_id.clone(),
                user_id: user_id.clone(),
                resuming: false,
                timeout: 60,
                players: Vec::new(),
            },
        );
    }

    state.plugin_manager.on_websocket_connect(&session_id).await;

    // Spawn a task to handle outgoing messages (NodeLink -> Client)
    let mut send_task = tokio::spawn(async move {
        let mut heartbeat = tokio::time::interval(tokio::time::Duration::from_secs(30));
        loop {
            tokio::select! {
                msg = rx.recv() => {
                    match msg {
                        Some(msg) => {
                            if let Err(e) = sender.send(Message::Text(msg.to_string())).await {
                                error!(target: "WebSocket", "Failed to send message: {}", e);
                                break;
                            }
                        }
                        None => break,
                    }
                }
                _ = heartbeat.tick() => {
                    if let Err(e) = sender.send(Message::Text(r#"{"op":"ping"}"#.into())).await {
                        error!(target: "WebSocket", "Failed to send heartbeat ping: {}", e);
                        break;
                    }
                }
            }
        }
    });

    let state_clone = state.clone();
    let session_id_clone = session_id.clone();

    // Handle incoming messages (Client -> NodeLink)
    let mut recv_task = tokio::spawn(async move {
        while let Some(msg) = receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    // Handle pong/heartbeat responses and parse command
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                        if val.get("op").and_then(|o| o.as_str()) == Some("pong")
                            || val.get("op").and_then(|o| o.as_str()) == Some("heartbeat")
                        {
                            trace!(target: "WebSocket", "Received heartbeat/pong");
                            continue;
                        }
                        // Try to parse the Lavalink JSON command
                        match serde_json::from_str::<LavalinkOp>(&text) {
                            Ok(op) => {
                                info!(target: "WebSocket", "Received OP payload: {:?}", op);
                                state_clone.plugin_manager.on_websocket_message(&session_id_clone, &val).await;
                                // Forward the command to the background worker!
                                handle_op(&state_clone, &session_id_clone, op).await;
                            }
                            Err(e) => {
                                warn!(
                                    target: "WebSocket",
                                    "Failed to parse Lavalink OP code: {} | Payload: {}",
                                    e, text
                                );
                            }
                        }
                    }
                }
                Ok(Message::Close(_)) => {
                    info!(target: "WebSocket", "Connection closed by client.");
                    break;
                }
                Err(e) => {
                    error!(target: "WebSocket", "Connection encountered an error: {}", e);
                    break;
                }
                _ => {}
            }
        }
    });

    // Wait until either task finishes (i.e. connection closes or errors out)
    tokio::select! {
        _ = (&mut send_task) => recv_task.abort(),
        _ = (&mut recv_task) => send_task.abort(),
    };

    // Cleanup when done
    {
        let mut ws_senders = state.ws_senders.write().await;
        ws_senders.remove(&session_id);
        state.sessions.remove(&session_id);
    }
    state.plugin_manager.on_websocket_close(&session_id, 1000, "connection closed").await;
}
