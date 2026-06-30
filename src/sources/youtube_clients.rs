use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientKind {
    Web,
    WebEmbedded,
    WebParentTools,
    WebRemix,
    Android,
    AndroidVR,
    Ios,
    Music,
    Tv,
    TvCast,
    TvEmbedded,
}

impl ClientKind {
    pub fn all() -> &'static [ClientKind] {
        &[
            ClientKind::Web,
            ClientKind::WebEmbedded,
            ClientKind::WebParentTools,
            ClientKind::WebRemix,
            ClientKind::Android,
            ClientKind::AndroidVR,
            ClientKind::Ios,
            ClientKind::Music,
            ClientKind::Tv,
            ClientKind::TvCast,
            ClientKind::TvEmbedded,
        ]
    }

    pub fn requires_player_script(&self) -> bool {
        matches!(self, ClientKind::Web | ClientKind::WebEmbedded | ClientKind::WebParentTools | ClientKind::Tv | ClientKind::TvEmbedded)
    }

    pub fn supports_oauth(&self) -> bool {
        matches!(self, ClientKind::Tv | ClientKind::TvCast | ClientKind::TvEmbedded)
    }

    pub fn client_id(&self) -> u32 {
        match self {
            ClientKind::Web => 1,
            ClientKind::WebEmbedded => 56,
            ClientKind::WebParentTools => 62,
            ClientKind::WebRemix => 67,
            ClientKind::Android => 3,
            ClientKind::AndroidVR => 28,
            ClientKind::Ios => 5,
            ClientKind::Music => 26,
            ClientKind::Tv => 7,
            ClientKind::TvCast => 59,
            ClientKind::TvEmbedded => 85,
        }
    }

    pub fn client_name(&self) -> &'static str {
        match self {
            ClientKind::Web => "WEB",
            ClientKind::WebEmbedded => "WEB_EMBEDDED_PLAYER",
            ClientKind::WebParentTools => "WEB_PARENT_TOOLS",
            ClientKind::WebRemix => "WEB_REMIX",
            ClientKind::Android => "ANDROID",
            ClientKind::AndroidVR => "ANDROID_VR",
            ClientKind::Ios => "IOS",
            ClientKind::Music => "ANDROID_MUSIC",
            ClientKind::Tv => "TVHTML5",
            ClientKind::TvCast => "TVHTML5_CAST",
            ClientKind::TvEmbedded => "TVHTML5_SIMPLY_EMBEDDED_PLAYER",
        }
    }

    pub fn client_version(&self) -> &'static str {
        match self {
            ClientKind::Web => "2.20260114.01.00",
            ClientKind::WebEmbedded => "1.20260128.01.00",
            ClientKind::WebParentTools => "1.20220918",
            ClientKind::WebRemix => "1.20260302.03.01",
            ClientKind::Android => "20.01.35",
            ClientKind::AndroidVR => "1.65.10",
            ClientKind::Ios => "21.02.1",
            ClientKind::Music => "8.47.54",
            ClientKind::Tv => "7.20260113.16.00",
            ClientKind::TvCast => "7.20190924",
            ClientKind::TvEmbedded => "2.0",
        }
    }

    pub fn user_agent(&self) -> &'static str {
        match self {
            ClientKind::Web => "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36",
            ClientKind::WebEmbedded => "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/144.0.0.0 Safari/537.36,gzip(gfe)",
            ClientKind::WebParentTools => "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36,gzip(gfe)",
            ClientKind::WebRemix => "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36",
            ClientKind::Android => "com.google.android.youtube/20.01.35 (Linux; U; Android 14) identity",
            ClientKind::AndroidVR => "Mozilla/5.0 (X11; Linux x86_64; Quest 3) AppleWebKit/537.36 (KHTML, like Gecko) OculusBrowser/39.3.0.11.46.766180192 Chrome/136.0.7103.177 VR Safari/537.36,gzip(gfe);GoogleHypersonic",
            ClientKind::Ios => "com.google.ios.youtube/21.02.1 (iPhone16,2; U; CPU iOS 18_2 like Mac OS X;)",
            ClientKind::Music => "com.google.android.apps.youtube.music/8.47.54 (Linux; U; Android 14 gzip)",
            ClientKind::Tv => "Mozilla/5.0 (Fuchsia) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36 CrKey/1.56.500000",
            ClientKind::TvCast => "Mozilla/5.0 (Linux; Android) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36 CrKey/1.54.248666",
            ClientKind::TvEmbedded => "Mozilla/5.0 (Linux armeabi-v7a; Android 7.1.2; Fire OS 6.0) Cobalt/22.lts.3.306369-gold (unlike Gecko) v8/8.8.278.8-jit gles Starboard/13, Amazon_ATV_mediatek8695_2019/NS6294 (Amazon, AFTMM, Wireless) com.amazon.firetv.youtube/22.3.r2.v66.0",
        }
    }

    pub fn build_context(&self, hl: &str, gl: &str, visitor_data: Option<&str>) -> Value {
        let mut client = json!({
            "clientName": self.client_name(),
            "clientVersion": self.client_version(),
            "hl": hl,
            "gl": gl,
        });

        let obj = client.as_object_mut().unwrap();

        match self {
            ClientKind::Android => {
                obj.insert("deviceMake".into(), json!("Google"));
                obj.insert("deviceModel".into(), json!("Pixel 6"));
                obj.insert("osName".into(), json!("Android"));
                obj.insert("osVersion".into(), json!("14"));
                obj.insert("androidSdkVersion".into(), json!("34"));
                obj.insert("userAgent".into(), json!(self.user_agent()));
                if let Some(vd) = visitor_data {
                    obj.insert("visitorData".into(), json!(vd));
                }
            }
            ClientKind::AndroidVR => {
                obj.insert("deviceMake".into(), json!("Google"));
                obj.insert("osName".into(), json!("Android"));
                obj.insert("osVersion".into(), json!("15"));
                obj.insert("androidSdkVersion".into(), json!("35"));
                obj.insert("userAgent".into(), json!(self.user_agent()));
                if let Some(vd) = visitor_data {
                    obj.insert("visitorData".into(), json!(vd));
                }
            }
            ClientKind::Ios => {
                obj.insert("deviceMake".into(), json!("Apple"));
                obj.insert("deviceModel".into(), json!("iPhone16,2"));
                obj.insert("osName".into(), json!("iPhone"));
                obj.insert("osVersion".into(), json!("18.2.22C152"));
                obj.insert("utcOffsetMinutes".into(), json!(0));
                obj.insert("userAgent".into(), json!(self.user_agent()));
                if let Some(vd) = visitor_data {
                    obj.insert("visitorData".into(), json!(vd));
                }
            }
            ClientKind::Music => {
                obj.insert("deviceMake".into(), json!("Google"));
                obj.insert("deviceModel".into(), json!("Pixel 6"));
                obj.insert("osName".into(), json!("Android"));
                obj.insert("osVersion".into(), json!("14"));
                obj.insert("androidSdkVersion".into(), json!("30"));
                obj.insert("userAgent".into(), json!(self.user_agent()));
            }
            ClientKind::Web | ClientKind::Tv => {
                obj.insert("platform".into(), json!("DESKTOP"));
                obj.insert("userAgent".into(), json!(self.user_agent()));
            }
            ClientKind::WebEmbedded => {
                obj.insert("platform".into(), json!("DESKTOP"));
                obj.insert("userAgent".into(), json!(self.user_agent()));
                if let Some(vd) = visitor_data {
                    obj.insert("visitorData".into(), json!(vd));
                }
            }
            ClientKind::WebParentTools => {
                obj.insert("userAgent".into(), json!(self.user_agent()));
                if let Some(vd) = visitor_data {
                    obj.insert("visitorData".into(), json!(vd));
                }
            }
            ClientKind::WebRemix => {
                obj.insert("userAgent".into(), json!(self.user_agent()));
                if let Some(vd) = visitor_data {
                    obj.insert("visitorData".into(), json!(vd));
                }
            }
            ClientKind::TvCast => {
                obj.insert("userAgent".into(), json!(self.user_agent()));
                if let Some(vd) = visitor_data {
                    obj.insert("visitorData".into(), json!(vd));
                }
            }
            ClientKind::TvEmbedded => {
                obj.insert("userAgent".into(), json!(self.user_agent()));
            }
        }

        let mut result = json!({
            "client": client,
            "user": { "lockedSafetyMode": false },
            "request": { "useSsl": true },
        });

        // Add thirdParty.embedUrl for embedded clients
        match self {
            ClientKind::WebEmbedded => {
                if let Some(obj) = result.as_object_mut() {
                    obj.insert("thirdParty".into(), json!({"embedUrl": "https://www.google.com/"}));
                }
            }
            ClientKind::WebParentTools => {
                if let Some(obj) = result.as_object_mut() {
                    obj.insert("thirdParty".into(), json!({"embedUrl": "https://www.youtube.com/"}));
                }
            }
            ClientKind::TvEmbedded => {
                if let Some(obj) = result.as_object_mut() {
                    obj.insert("thirdParty".into(), json!({"embedUrl": "https://www.youtube.com/tv"}));
                }
            }
            _ => {}
        }

        result
    }

    pub fn api_endpoint(&self) -> &'static str {
        match self {
            ClientKind::Web => "https://www.youtube.com",
            ClientKind::WebEmbedded => "https://www.youtube.com",
            ClientKind::WebParentTools => "https://www.youtube.com",
            ClientKind::WebRemix => "https://music.youtube.com",
            ClientKind::Android => "https://youtubei.googleapis.com",
            ClientKind::AndroidVR => "https://youtubei.googleapis.com",
            ClientKind::Ios => "https://youtubei.googleapis.com",
            ClientKind::Music => "https://music.youtube.com",
            ClientKind::Tv => "https://youtubei.googleapis.com",
            ClientKind::TvCast => "https://youtubei.googleapis.com",
            ClientKind::TvEmbedded => "https://youtubei.googleapis.com",
        }
    }

    pub fn search_params(&self, search_type: &str) -> Option<&'static str> {
        match self {
            ClientKind::Web => Some("EgIQAQ%3D%3D"),
            ClientKind::WebEmbedded => Some("EgVo2aDSNQ=="),
            ClientKind::Android => match search_type {
                "tracks" => Some("EgIQAQ%3D%3D"),
                "playlists" => Some("EgIQAw%3D%3D"),
                "artists" => Some("EgIQAg%3D%3D"),
                _ => Some("EgIQAQ%3D%3D"),
            },
            ClientKind::WebRemix => match search_type {
                "tracks" => Some("EgWKAQIIAWoSEAMQBRAEEAkQChAVEBAQDhAR"),
                "playlists" => Some("EgeKAQQoAEABahIQAxAFEAQQCRAKEBUQEBAOEBE%3D"),
                "albums" => Some("EgWKAQIYAWoSEAMQBRAEEAkQChAVEBAQDhAR"),
                "artists" => Some("EgWKAQIgAWoSEAMQBRAEEAkQChAVEBAQDhAR"),
                _ => None,
            },
            ClientKind::Music => match search_type {
                "tracks" => Some("EgWKAQIIAWoQEAMQBBAJEAoQBRAREBAQFQ%3D%3D"),
                "playlists" => Some("EgWKAQIoAWoKEAMQBBAJEAoQBRAB"),
                "albums" => Some("EgWKAQIYAWoKEAMQBBAJEAoQBRAB"),
                "artists" => Some("EgWKAQIYAWoKEAMQBBAJEAoQBRAB"),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn player_params(&self) -> Option<&'static str> {
        match self {
            ClientKind::TvEmbedded => Some("2AMB"),
            _ => None,
        }
    }

    pub fn sabr_client_version(&self) -> Option<&'static str> {
        match self {
            ClientKind::Android => Some("20.51.39"),
            _ => None,
        }
    }

    pub fn search_headers(&self) -> Vec<(&'static str, &'static str)> {
        match self {
            ClientKind::Web => vec![("X-Goog-Api-Format-Version", "2")],
            ClientKind::WebEmbedded => vec![("X-Goog-Api-Format-Version", "2")],
            ClientKind::WebParentTools => vec![
                ("X-YouTube-Client-Name", "88"),
                ("X-YouTube-Client-Version", self.client_version()),
            ],
            ClientKind::Android => vec![
                ("X-Goog-Api-Format-Version", "2"),
                ("X-YouTube-Client-Name", "3"),
                ("X-YouTube-Client-Version", self.client_version()),
            ],
            ClientKind::WebRemix => vec![("X-Goog-Api-Format-Version", "2")],
            ClientKind::Music => vec![("X-Goog-Api-Format-Version", "2")],
            _ => vec![],
        }
    }

    pub fn can_provide_track_url(&self) -> bool {
        !matches!(self, ClientKind::Music | ClientKind::WebRemix)
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            ClientKind::Web => "WEB",
            ClientKind::WebEmbedded => "WEB Embedded",
            ClientKind::WebParentTools => "WEB Parent Tools",
            ClientKind::WebRemix => "WEB Remix (YouTube Music)",
            ClientKind::Android => "Android",
            ClientKind::AndroidVR => "Android VR",
            ClientKind::Ios => "iOS",
            ClientKind::Music => "YouTube Music (Android)",
            ClientKind::Tv => "TV",
            ClientKind::TvCast => "TV Cast",
            ClientKind::TvEmbedded => "TV Embedded",
        }
    }
}
