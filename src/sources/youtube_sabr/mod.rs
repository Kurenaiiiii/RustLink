pub mod protor;
pub mod stream;

use std::sync::Arc;
use tokio::sync::Mutex;
use reqwest::Client as HttpClient;

pub use protor::*;
pub use stream::*;

pub struct SabrConfig {
    pub http_client: HttpClient,
}

impl Default for SabrConfig {
    fn default() -> Self {
        Self {
            http_client: HttpClient::new(),
        }
    }
}

pub struct SabrManager {
    config: Arc<Mutex<SabrConfig>>,
}

impl SabrManager {
    pub fn new(http_client: HttpClient) -> Self {
        Self {
            config: Arc::new(Mutex::new(SabrConfig { http_client })),
        }
    }

    pub async fn resolve_sabr_url(
        &self,
        video_id: &str,
        server_abr_streaming_url: &str,
        ustreamer_config: &[u8],
        client_name: i32,
        client_version: &str,
        formats: Vec<FormatEntry>,
        po_token: Option<Vec<u8>>,
        visitor_data: Option<String>,
        start_time: i64,
    ) -> Result<SabrStream, String> {
        let cfg = self.config.lock().await;
        let http_client = cfg.http_client.clone();

        let config = SabrStreamConfig {
            video_id: video_id.to_string(),
            server_abr_streaming_url: Some(server_abr_streaming_url.to_string()),
            video_playback_ustreamer_config: Some(ustreamer_config.to_vec()),
            client_info: Some(ClientInfoMsg {
                client_name,
                client_version: client_version.to_string(),
            }),
            formats,
            po_token,
            visitor_data,
            start_time,
            user_agent: None,
            previous_session: None,
            access_token: None,
        };

        let stream = SabrStream::new(http_client, config);
        Ok(stream)
    }

    pub async fn sabr_config(&self) -> tokio::sync::MutexGuard<'_, SabrConfig> {
        self.config.lock().await
    }
}
