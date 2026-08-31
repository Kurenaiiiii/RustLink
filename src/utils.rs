use std::time::{SystemTime, UNIX_EPOCH};

/// Parsed semantic version components.
#[derive(Debug, Clone)]
pub struct SemverInfo {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
    pub prerelease: Vec<String>,
    pub build: Vec<String>,
}

/// Client information parsed from the Client-Name header.
#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub name: String,
    pub version: Option<String>,
    pub url: Option<String>,
    pub codename: Option<String>,
    pub release_date: Option<String>,
}

/// Git repository metadata.
#[derive(Debug, Clone)]
pub struct GitInfo {
    pub branch: String,
    pub commit: String,
    pub commit_time: i64,
}

/// Options for best-match track scoring.
#[derive(Debug, Clone)]
pub struct BestMatchOptions {
    pub duration_tolerance: f32,
    pub allow_explicit: bool,
}

impl Default for BestMatchOptions {
    fn default() -> Self {
        Self { duration_tolerance: 0.15, allow_explicit: true }
    }
}

/// Track info used for best-match scoring.
#[derive(Debug, Clone)]
pub struct BestMatchTrackInfo {
    pub title: String,
    pub author: String,
    pub length: i64,
    pub uri: Option<String>,
}

/// Candidate track for best-match scoring.
#[derive(Debug, Clone)]
pub struct BestMatchCandidate {
    pub info: BestMatchTrackInfo,
}

/// Validates a Discord snowflake ID format.
pub fn verify_discord_id(id: &str) -> bool {
    if id.is_empty() || id.len() > 19 { return false; }
    id.chars().all(|c| c.is_ascii_digit())
}

/// Validates a configuration property.
pub fn validate_property<T: std::fmt::Debug>(
    value: Option<&T>,
    path: &str,
    expected: &str,
    validator: fn(&T) -> bool,
) -> Result<(), String> {
    match value {
        None => Err(format!(
            "Configuration error:\n- Property: {path}\n- Received: undefined\n- Problem: missing required value\n- Expected: {expected}\n\nPlease define {path} in your config."
        )),
        Some(v) if !validator(v) => Err(format!(
            "Configuration error:\n- Property: {path}\n- Received: {v:?}\n- Expected: {expected}"
        )),
        _ => Ok(()),
    }
}

/// Parses a semantic version string.
pub fn parse_semver(version: &str) -> Option<SemverInfo> {
    let re = regex::Regex::new(
        r"^(?P<major>\d+)\.(?P<minor>\d+)\.(?P<patch>\d+)(?:-(?P<prerelease>[0-9A-Za-z.-]+))?(?:\+(?P<build>[0-9A-Za-z.-]+))?$"
    ).ok()?;
    let caps = re.captures(version)?;
    Some(SemverInfo {
        major: caps["major"].parse().ok()?,
        minor: caps["minor"].parse().ok()?,
        patch: caps["patch"].parse().ok()?,
        prerelease: caps.name("prerelease").map(|m| m.as_str().split('.').map(String::from).collect()).unwrap_or_default(),
        build: caps.name("build").map(|m| m.as_str().split('.').map(String::from).collect()).unwrap_or_default(),
    })
}

/// Returns the current application version.
pub fn get_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Returns parsed semver of the current application version.
pub fn get_version_object() -> Option<SemverInfo> {
    parse_semver(env!("CARGO_PKG_VERSION"))
}

/// Parses the `Client-Name` header into structured info.
pub fn parse_client(agent: Option<&str>) -> Option<ClientInfo> {
    let agent = agent?.trim();
    if agent.is_empty() { return None; }

    let mut parts = agent.splitn(2, ' ');
    let core = parts.next()?;
    let meta_part = parts.next();

    let mut core_parts = core.splitn(2, '/');
    let name = core_parts.next()?.to_string();
    let version = core_parts.next().map(String::from);

    let mut info = ClientInfo { name, version, url: None, codename: None, release_date: None };

    if let Some(meta) = meta_part {
        if meta.starts_with('(') && meta.ends_with(')') {
            let inner = &meta[1..meta.len() - 1];
            if inner.starts_with("http") {
                info.url = Some(inner.to_string());
            } else if let Some(slash_pos) = inner.find('/') {
                info.codename = Some(inner[..slash_pos].to_string());
                info.release_date = Some(inner[slash_pos + 1..].to_string());
            }
        }
    }

    Some(info)
}

/// Returns cached or fresh git repository metadata.
pub fn get_git_info() -> GitInfo {
    let branch = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
            } else { None }
        })
        .unwrap_or_else(|| "unknown".to_string());

    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
            } else { None }
        })
        .unwrap_or_else(|| "unknown".to_string());

    let commit_time = std::process::Command::new("git")
        .args(["log", "-1", "--format=%ct"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
                    .and_then(|s| s.trim().parse::<i64>().ok())
                    .map(|ts| ts * 1000)
            } else { None }
        })
        .unwrap_or(-1);

    GitInfo { branch, commit, commit_time }
}

/// Applies environment variable overrides onto a config map.
pub fn apply_env_overrides(config: &mut serde_json::Value, prefix: &str) {
    match config {
        serde_json::Value::Object(map) => {
            let keys: Vec<String> = map.keys().cloned().collect();
            for key in keys {
                let env_name = format!("{}_{}", prefix, key.to_uppercase());
                let is_obj = map[&key].is_object();
                if let Ok(val) = std::env::var(&env_name) {
                    let current = &map[&key];
                    match current {
                        serde_json::Value::Bool(_) => {
                            map.insert(key, serde_json::Value::Bool(val.to_lowercase() == "true"));
                        }
                        serde_json::Value::Number(_) => {
                            if let Ok(n) = val.parse::<f64>() {
                                map.insert(key, serde_json::Value::from(n));
                            }
                        }
                        serde_json::Value::String(_) => {
                            map.insert(key, serde_json::Value::String(val));
                        }
                        serde_json::Value::Array(_) => {
                            let new_arr: Vec<serde_json::Value> = val
                                .split(',')
                                .map(|s| serde_json::Value::String(s.trim().to_string()))
                                .filter(|v| !v.as_str().map_or(true, |s| s.is_empty()))
                                .collect();
                            if !new_arr.is_empty() {
                                map.insert(key, serde_json::Value::Array(new_arr));
                            }
                        }
                        serde_json::Value::Object(_) => {
                            apply_env_overrides(&mut map[&key], &env_name);
                        }
                        _ => {}
                    }
                } else if is_obj {
                    apply_env_overrides(&mut map[&key], &env_name);
                }
            }
        }
        _ => {}
    }
}

/// Checks for git updates against the upstream branch.
pub fn check_for_updates() {
    let _ = std::process::Command::new("git")
        .args(["fetch"])
        .output();

    let local = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string()));

    let remote = std::process::Command::new("git")
        .args(["rev-parse", "@{u}"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string()));

    match (local, remote) {
        (Some(l), Some(r)) if l != r => {
            let behind = std::process::Command::new("git")
                .args(["rev-list", "--right-only", "--count", "HEAD...@{u}"])
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string()));
            let msg = format!(
                "Your version is {} commits behind the remote. Run \"git pull\" to update.",
                behind.as_deref().unwrap_or("unknown")
            );
            tracing::warn!("[Git]: {}", msg);
        }
        (Some(_), Some(_)) => {
            tracing::info!("[Git]: You are running the latest version.");
        }
        _ => {
            tracing::warn!("[Git]: Failed to check for updates.");
        }
    }
}

/// Selects the best match from a list of track candidates using scoring.
pub fn get_best_match<'a>(
    list: &'a [BestMatchCandidate],
    original: &'a BestMatchTrackInfo,
    options: &'a BestMatchOptions,
) -> Option<&'a BestMatchCandidate> {
    if list.is_empty() { return None; }

    let normalize = |s: &str| -> String {
        s.to_lowercase()
            .replace("feat.", "")
            .replace("ft.", "")
            .split(&['(', '['][..])
            .next()
            .unwrap_or("")
            .split(|c: char| !c.is_alphanumeric() && c != ' ')
            .collect::<Vec<&str>>()
            .join(" ")
            .split_whitespace()
            .filter(|w| w.len() > 1)
            .collect::<Vec<&str>>()
            .join(" ")
    };

    let spec_keywords = [
        "remix", "orchestral", "live", "cover", "acoustic", "instrumental",
        "karaoke", "radio", "edit", "extended", "slowed", "reverb",
    ];

    let find_spec = |s: &str| -> Vec<&str> {
        let lower = s.to_lowercase();
        spec_keywords.iter().filter(|&&k| lower.contains(k)).copied().collect()
    };

    let original_title = original.title.to_lowercase();
    let original_spec = find_spec(&original_title);
    let is_original_explicit = original.uri.as_deref().map_or(false, |u| u.contains("explicit=true"))
        || original_title.contains("explicit");

    let target_duration = original.length;
    let allowed_diff = (target_duration as f32 * options.duration_tolerance) as i64;
    let norm_original_author = normalize(&original.author);
    let original_words: Vec<String> = normalize(&original.title)
        .split_whitespace()
        .filter(|w| w.len() > 1)
        .map(String::from)
        .collect();

    let mut scored: Vec<(&BestMatchCandidate, i32)> = list.iter().map(|item| {
        let item_title = item.info.title.to_lowercase();
        let norm_item_title = normalize(&item.info.title);
        let norm_item_author = normalize(&item.info.author);
        let item_spec = find_spec(&item_title);
        let is_item_clean = item_title.contains("clean") || item_title.contains("radio edit");
        let mut score = 0i32;

        let item_words: Vec<String> = norm_item_title
            .split_whitespace()
            .filter(|w| w.len() > 1)
            .map(String::from)
            .collect();

        let overlap = original_words.iter().filter(|w| item_words.contains(w)).count();
        score += (overlap as i32 * 300) / original_words.len().max(1) as i32;

        for &spec in &spec_keywords {
            let in_original = original_spec.contains(&spec);
            let in_item = item_spec.contains(&spec);
            if in_original && in_item { score += 200; }
            if in_original != in_item { score -= 300; }
        }

        if is_original_explicit && !options.allow_explicit && is_item_clean {
            score += 500;
        }

        if norm_item_author.contains(&norm_original_author) || norm_original_author.contains(&norm_item_author) {
            score += 150;
        } else {
            let (longer, shorter) = if norm_original_author.len() > norm_item_author.len() {
                (&norm_original_author, &norm_item_author)
            } else {
                (&norm_item_author, &norm_original_author)
            };
            if shorter.len() > 2 && longer.contains(shorter) { score += 100; }
        }

        if target_duration > 0 {
            let diff = (item.info.length - target_duration).abs();
            if diff <= allowed_diff {
                let ratio = 1.0 - diff as f32 / allowed_diff as f32;
                score += (ratio * 100.0) as i32;
            } else {
                score -= 100;
            }
        }

        if item_title.contains("official audio") || item_title.contains("topic") {
            score += 50;
        }

        (item, score)
    }).collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1));
    Some(scored.first()?.0)
}

/// Returns the current timestamp in milliseconds.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verify_discord_id() {
        assert!(verify_discord_id("123456789012345678"));
        assert!(verify_discord_id("0"));
        assert!(!verify_discord_id(""));
        assert!(!verify_discord_id("abc123"));
        assert!(!verify_discord_id("12345678901234567890"));
    }

    #[test]
    fn test_parse_semver() {
        let v = parse_semver("1.2.3").unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);

        let v = parse_semver("3.8.0-beta.1+build.123").unwrap();
        assert_eq!(v.major, 3);
        assert_eq!(v.prerelease, vec!["beta", "1"]);
        assert_eq!(v.build, vec!["build", "123"]);

        assert!(parse_semver("invalid").is_none());
    }

    #[test]
    fn test_get_version() {
        let v = get_version();
        assert!(!v.is_empty());
        let parsed = parse_semver(v);
        assert!(parsed.is_some(), "version {v} should be valid semver");
    }

    #[test]
    fn test_parse_client() {
        let c = parse_client(Some("Name/1.0")).unwrap();
        assert_eq!(c.name, "Name");
        assert_eq!(c.version, Some("1.0".into()));

        let c = parse_client(Some("Name/1.0 (codename/2024-01-01)")).unwrap();
        assert_eq!(c.codename, Some("codename".into()));
        assert_eq!(c.release_date, Some("2024-01-01".into()));

        let c = parse_client(Some("Name/1.0 (https://example.com)")).unwrap();
        assert_eq!(c.url, Some("https://example.com".into()));

        assert!(parse_client(Some("")).is_none());
        assert!(parse_client(None).is_none());
    }

    #[test]
    fn test_best_match_exact() {
        let original = BestMatchTrackInfo {
            title: "Song Title".into(),
            author: "Artist".into(),
            length: 200_000,
            uri: None,
        };
        let candidates = vec![
            BestMatchCandidate { info: BestMatchTrackInfo {
                title: "Wrong Song".into(), author: "Other".into(), length: 180_000, uri: None,
            }},
            BestMatchCandidate { info: BestMatchTrackInfo {
                title: "Song Title".into(), author: "Artist".into(), length: 200_000, uri: None,
            }},
        ];
        let options = BestMatchOptions::default();
        let result = get_best_match(&candidates, &original, &options);
        assert!(result.is_some());
        assert_eq!(result.unwrap().info.title, "Song Title");
    }

    #[test]
    fn test_best_match_empty() {
        let options = BestMatchOptions::default();
        assert!(get_best_match(&[], &BestMatchTrackInfo {
            title: "".into(), author: "".into(), length: 0, uri: None,
        }, &options).is_none());
    }

    #[test]
    fn test_validate_property() {
        assert!(validate_property(Some(&"hello"), "test", "string", |s: &&str| !s.is_empty()).is_ok());
        assert!(validate_property::<String>(None, "test", "string", |_| true).is_err());
    }
}
