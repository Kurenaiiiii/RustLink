use std::sync::Arc;
use dashmap::DashMap;
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::state::Player;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub user_id: String,
    pub resuming: bool,
    pub timeout_secs: u64,
    pub players: Vec<Player>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionInfo {
    pub id: String,
    pub user_id: String,
    pub resuming: bool,
    pub timeout_secs: u64,
}

pub struct SessionManager {
    active_sessions: Arc<DashMap<String, Session>>,
    resumable_sessions: Arc<DashMap<String, Session>>,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            active_sessions: Arc::new(DashMap::new()),
            resumable_sessions: Arc::new(DashMap::new()),
        }
    }

    pub async fn create(&self, user_id: String) -> String {
        let id = generate_random_id(16);
        let session = Session {
            id: id.clone(),
            user_id,
            resuming: false,
            timeout_secs: 60,
            players: Vec::new(),
        };
        self.active_sessions.insert(id.clone(), session);
        id
    }

    pub fn get(&self, session_id: &str) -> Option<Session> {
        self.active_sessions
            .get(session_id)
            .map(|s| s.value().clone())
            .or_else(|| {
                self.resumable_sessions
                    .get(session_id)
                    .map(|s| s.value().clone())
            })
    }

    pub fn get_active(&self, session_id: &str) -> Option<Session> {
        self.active_sessions
            .get(session_id)
            .map(|s| s.value().clone())
    }

    pub fn has(&self, session_id: &str) -> bool {
        self.active_sessions.contains_key(session_id)
            || self.resumable_sessions.contains_key(session_id)
    }

    pub async fn pause(&self, session_id: &str) {
        if self.resumable_sessions.contains_key(session_id) {
            return;
        }

        if let Some((_, session)) = self.active_sessions.remove(session_id) {
            let mut paused = session.clone();
            paused.resuming = true;
            self.resumable_sessions
                .insert(session_id.to_string(), paused);
        }
    }

    pub async fn resume(&self, session_id: &str) -> Option<Session> {
        if let Some((_, session)) = self.resumable_sessions.remove(session_id) {
            let mut resumed = session.clone();
            resumed.resuming = false;
            self.active_sessions.insert(session_id.to_string(), resumed.clone());
            Some(resumed)
        } else {
            None
        }
    }

    pub async fn destroy(&self, session_id: &str) {
        self.active_sessions.remove(session_id);
        self.resumable_sessions.remove(session_id);
    }

    pub fn active_count(&self) -> usize {
        self.active_sessions.len()
    }

    pub fn resumable_count(&self) -> usize {
        self.resumable_sessions.len()
    }

    pub fn active_sessions(&self) -> Vec<Session> {
        self.active_sessions
            .iter()
            .map(|s| s.value().clone())
            .collect()
    }

    pub async fn get_player(&self, guild_id: &str) -> Option<Player> {
        for session in self.active_sessions.iter() {
            for player in &session.players {
                if player.guild_id == guild_id {
                    return Some(player.clone());
                }
            }
        }
        None
    }

    pub async fn all_players(&self) -> Vec<Player> {
        let mut players = Vec::new();
        for session in self.active_sessions.iter() {
            players.extend(session.players.clone());
        }
        players
    }
}

fn generate_random_id(length: usize) -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..length)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}
