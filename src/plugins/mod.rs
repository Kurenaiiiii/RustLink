use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;
use libloading::{Library, Symbol};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::config::PluginsConfig;

// ---------------------------------------------------------------------------
// Types matching NodeLink's plugin system
// ---------------------------------------------------------------------------

/// Process context in which a plugin boots (mirrors `PluginContextType`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PluginContextType {
    #[serde(rename = "master")]
    Master,
    #[serde(rename = "worker")]
    Worker,
    #[serde(rename = "voice-worker")]
    VoiceWorker,
    #[serde(rename = "source-worker")]
    SourceWorker,
    #[serde(rename = "micro-worker")]
    MicroWorker,
}

impl PluginContextType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Master => "master",
            Self::Worker => "worker",
            Self::VoiceWorker => "voice-worker",
            Self::SourceWorker => "source-worker",
            Self::MicroWorker => "micro-worker",
        }
    }
}

/// Metadata about a plugin (mirrors `PluginMeta`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMeta {
    pub name: String,
    pub version: String,
    pub author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
}

/// Execution context passed to every plugin (mirrors `PluginExecutionContext`).
#[derive(Debug, Clone)]
pub struct PluginExecutionContext {
    pub context_type: PluginContextType,
    pub worker_id: String,
    pub plugin_name: String,
    pub meta: PluginMeta,
}

/// Plugin definition as declared in config (mirrors `PluginDefinition`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginDefinition {
    pub name: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<serde_json::Value>,
}

/// Internal loaded-plugin entry (mirrors `LoadedPluginEntry`).
struct PluginEntry {
    plugin: Box<dyn Plugin>,
    path: String,
    meta: PluginMeta,
    _lib: Option<Box<Library>>, // Keep library alive for dynamically loaded plugins
}

// ---------------------------------------------------------------------------
// Hook infrastructure
// ---------------------------------------------------------------------------

/// Hook names matching NodeLink's `PluginHookName`.
pub mod hook_names {
    pub const ON_PLAYER_CREATE: &str = "onPlayerCreate";
    pub const ON_PLAYER_DESTROY: &str = "onPlayerDestroy";
    pub const ON_TRACK_START: &str = "onTrackStart";
    pub const ON_TRACK_END: &str = "onTrackEnd";
    pub const ON_TRACK_EXCEPTION: &str = "onTrackException";
    pub const ON_TRACK_STUCK: &str = "onTrackStuck";
    pub const ON_SEARCH: &str = "onSearch";
    pub const ON_RESOLVE: &str = "onResolve";
    pub const ON_REST_REQUEST: &str = "onRESTRequest";
    pub const ON_WEB_SOCKET_CONNECT: &str = "onWebSocketConnect";
    pub const ON_WEB_SOCKET_MESSAGE: &str = "onWebSocketMessage";
    pub const ON_WEB_SOCKET_CLOSE: &str = "onWebSocketClose";
    pub const ON_IPC_MESSAGE: &str = "onIPCMessage";
}

type HookCallback = Box<dyn Fn(&[&dyn std::any::Any]) + Send + Sync>;

// ---------------------------------------------------------------------------
// Plugin trait (matching NodeLink's PluginExecutor pattern)
// ---------------------------------------------------------------------------

/// The base trait every RustLink plugin must implement.
///
/// Analogous to NodeLink's `PluginExecutor` function — instead of passing
/// a free function, each plugin implements this trait and provides hook
/// callbacks.
#[async_trait]
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str {
        "1.0.0"
    }
    fn author(&self) -> &str {
        "unknown"
    }
    fn topic(&self) -> Option<&str> {
        None
    }

    /// Called once after loading to pass context and per-plugin config.
    /// Plugins should register hooks and set up interceptors here.
    async fn init(&self, _ctx: &PluginExecutionContext, _config: &serde_json::Value) {}

    // ----- Player lifecycle hooks -----

    async fn on_player_create(&self, _guild_id: &str, _session_id: &str, _result: &serde_json::Value) {}

    async fn on_player_destroy(&self, _guild_id: &str, _session_id: &str) {}

    // ----- Track lifecycle hooks -----

    async fn on_track_start(&self, _guild_id: &str, _track: &serde_json::Value) {}

    async fn on_track_end(&self, _guild_id: &str, _track: &serde_json::Value, _reason: &str) {}

    async fn on_track_exception(&self, _guild_id: &str, _track: &serde_json::Value, _exception: &serde_json::Value) {}

    async fn on_track_stuck(&self, _guild_id: &str, _track: &serde_json::Value, _threshold_ms: u64) {}

    // ----- Source hooks -----

    async fn on_search(&self, _query: &str, _source_name: &str, _search_type: &str) {}

    async fn on_resolve(&self, _url: &str, _source_name: &str) {}

    // ----- WebSocket hooks -----

    async fn on_websocket_connect(&self, _session_id: &str) {}

    async fn on_websocket_close(&self, _session_id: &str, _code: u16, _reason: &str) {}

    async fn on_websocket_message(&self, _session_id: &str, _message: &serde_json::Value) {}

    // ----- Player update hooks -----

    async fn on_player_update(&self, _guild_id: &str, _state: &serde_json::Value) {}

    async fn on_voice_server_update(&self, _guild_id: &str, _endpoint: &str, _token: &str) {}

    // ----- Audio hooks -----

    async fn on_audio_packet(&self, _guild_id: &str, _packet: &[u8]) {}

    // ----- Generic event hook -----

    async fn on_event(&self, _guild_id: &str, _event: &serde_json::Value) {}

    // ----- REST hook -----

    async fn on_rest_request(&self, _method: &str, _path: &str, _headers: &serde_json::Value) {}

    // ----- IPC hook -----

    async fn on_ipc_message(&self, _message: &serde_json::Value) {}
}

// ---------------------------------------------------------------------------
// Dynamic plugin loading (libloading-based)
// ---------------------------------------------------------------------------

struct DynamicPlugin {
    inner: Box<dyn Plugin>,
    _lib: Library,
}

impl DynamicPlugin {
    fn new(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        unsafe {
            let lib = Library::new(path)?;
            let create: Symbol<unsafe extern "C" fn() -> *mut dyn Plugin> =
                lib.get(b"rustlink_plugin_create")?;
            let inner = Box::from_raw(create());
            Ok(Self { inner, _lib: lib })
        }
    }
}

// ---------------------------------------------------------------------------
// PluginManager (matching NodeLink's PluginManager)
// ---------------------------------------------------------------------------

pub struct PluginManager {
    /// Loaded plugin entries keyed by plugin name.
    loaded_plugins: HashMap<String, PluginEntry>,
    /// Custom hooks registered by plugins themselves.
    hooks: HashMap<String, Vec<HookCallback>>,
    /// Plugin definitions from config.
    definitions: Vec<PluginDefinition>,
    /// Per-plugin config.
    plugin_configs: HashMap<String, serde_json::Value>,
    /// Base plugins directory.
    plugins_dir: String,
}

impl PluginManager {
    /// Create a new empty PluginManager.
    pub fn new() -> Self {
        Self {
            loaded_plugins: HashMap::new(),
            hooks: HashMap::new(),
            definitions: Vec::new(),
            plugin_configs: HashMap::new(),
            plugins_dir: "plugins".into(),
        }
    }

    /// Set the plugin definitions (from config).
    pub fn set_definitions(&mut self, definitions: Vec<PluginDefinition>) {
        self.definitions = definitions;
    }

    /// Set the plugins base directory.
    pub fn set_plugins_dir(&mut self, dir: &str) {
        self.plugins_dir = dir.into();
    }

    /// Load all configured plugins for the given context type.
    ///
    /// Mirrors NodeLink's `PluginManager.load(contextType)`.
    pub async fn load(&mut self, context_type: PluginContextType, worker_id: &str) {
        for def in &self.definitions {
            if def.name.is_empty() {
                continue;
            }

            // Already loaded? Just re-init (NodeLink re-executes cached modules).
            if self.loaded_plugins.contains_key(&def.name) {
                info!(target: "Plugins", "Plugin '{}' already loaded, re-executing for {}", def.name, context_type.as_str());
                self.execute_plugin(&def.name, &context_type, worker_id).await;
                continue;
            }

            // Resolve entry point
            let entry_path = match self.resolve_entry(def) {
                Some(p) => p,
                None => {
                    warn!(target: "Plugins", "Could not resolve entry for plugin '{}'", def.name);
                    continue;
                }
            };

            // Extract metadata from package.json (or fallback to trait methods)
            let meta = self.extract_metadata(&entry_path, def);

            // Load the plugin module
            let (plugin, lib_handle): (Box<dyn Plugin>, Option<Box<Library>>) = match self.load_module(&entry_path) {
                Ok(r) => r,
                Err(e) => {
                    warn!(target: "Plugins", "Failed to load plugin '{}' from {}: {}", def.name, entry_path, e);
                    continue;
                }
            };

            // Cache
            self.loaded_plugins.insert(
                def.name.clone(),
                PluginEntry {
                    plugin,
                    path: entry_path.clone(),
                    meta: meta.clone(),
                    _lib: lib_handle,
                },
            );

            // Store per-plugin config
            if let Some(cfg) = &def.config {
                self.plugin_configs.insert(def.name.clone(), cfg.clone());
            }

            // Execute (call init)
            self.execute_plugin(&def.name, &context_type, worker_id).await;

            info!(
                target: "Plugins",
                "Loaded plugin: {} v{} by {} [{}]",
                meta.name, meta.version, meta.author, entry_path
            );
        }
    }

    /// Register a loaded plugin directly (for built-in plugins).
    pub fn register(&mut self, name: &str, plugin: Box<dyn Plugin>) {
        let meta = PluginMeta {
            name: name.to_string(),
            version: plugin.version().to_string(),
            author: plugin.author().to_string(),
            topic: plugin.topic().map(|s| s.to_string()),
        };
        info!(target: "Plugins", "Registered built-in plugin: {} v{} by {}", name, meta.version, meta.author);
        self.loaded_plugins.insert(
            name.to_string(),
            PluginEntry {
                plugin,
                path: String::new(),
                meta,
                _lib: None,
            },
        );
    }

    /// Load a dynamic plugin from a shared library path.
    pub fn load_dynamic(&mut self, name: &str, path: &str) {
        match DynamicPlugin::new(path) {
            Ok(dp) => {
                let meta = PluginMeta {
                    name: name.to_string(),
                    version: dp.inner.version().to_string(),
                    author: dp.inner.author().to_string(),
                    topic: dp.inner.topic().map(|s| s.to_string()),
                };
                let lib = Box::new(dp._lib);
                info!(target: "Plugins", "Loaded dynamic plugin: {} v{} by {} from {}", name, meta.version, meta.author, path);
                self.loaded_plugins.insert(
                    name.to_string(),
                    PluginEntry {
                        plugin: dp.inner,
                        path: path.to_string(),
                        meta,
                        _lib: Some(lib),
                    },
                );
            }
            Err(e) => {
                error!(target: "Plugins", "Failed to load dynamic plugin from {}: {}", path, e);
            }
        }
    }

    /// Load plugins from a `PluginsConfig` (legacy path-based config).
    pub fn load_from_config(&mut self, config: &PluginsConfig) {
        if !config.enabled {
            return;
        }
        // If definitions are present, they get loaded via `load()`.
        // The legacy `paths` are loaded as dynamic plugins.
        for (i, path) in config.paths.iter().enumerate() {
            let stem = Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("plugin-{}", i));
            self.load_dynamic(&stem, path);
        }
    }

    // ------------------------------------------------------------------
    // Hook registration and invocation (mirrors NodeLink's API)
    // ------------------------------------------------------------------

    /// Register a callback for a named hook (mirrors `registerHook`).
    pub fn register_hook<F>(&mut self, name: &str, callback: F)
    where
        F: Fn(&[&dyn std::any::Any]) + Send + Sync + 'static,
    {
        self.hooks.entry(name.to_string()).or_default().push(Box::new(callback));
    }

    /// Synchronously call all registered callbacks for a hook (mirrors `callHook`).
    pub fn call_hook(&self, name: &str, args: &[&dyn std::any::Any]) {
        let Some(callbacks) = self.hooks.get(name) else { return };
        for cb in callbacks {
            (cb)(args);
        }
    }

    /// Asynchronously call all registered callbacks (mirrors `callHookAsync`).
    pub async fn call_hook_async(&self, name: &str, args: &[&dyn std::any::Any]) {
        let Some(callbacks) = self.hooks.get(name) else { return };
        for cb in callbacks {
            (cb)(args);
        }
    }

    // ------------------------------------------------------------------
    // Metadata helpers
    // ------------------------------------------------------------------

    /// Returns plugin metadata for the `/v4/info` endpoint.
    pub fn get_loaded_plugins(&self) -> Vec<serde_json::Value> {
        self.loaded_plugins
            .iter()
            .map(|(name, entry)| {
                serde_json::json!({
                    "name": name,
                    "version": entry.meta.version,
                    "author": entry.meta.author,
                    "path": if entry.path.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(entry.path.clone()) }
                })
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.loaded_plugins.is_empty()
    }

    pub fn len(&self) -> usize {
        self.loaded_plugins.len()
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    /// Resolve the filesystem entry point for a plugin definition.
    fn resolve_entry(&self, def: &PluginDefinition) -> Option<String> {
        // Direct path takes precedence
        if let Some(path) = &def.path {
            let resolved = if Path::new(path).is_absolute() {
                path.clone()
            } else {
                Path::new(&self.plugins_dir).join(path).to_string_lossy().to_string()
            };
            if Path::new(&resolved).exists() {
                return Some(resolved);
            }
            warn!(target: "Plugins", "Plugin path does not exist: {}", resolved);
        }

        // Local source: look in plugins/<name>/
        if def.source.as_deref() == Some("local") || def.path.is_none() {
            let dir = Path::new(&self.plugins_dir).join(&def.name);
            if dir.is_dir() {
                // Try package.json -> main
                let pkg_path = dir.join("package.json");
                if pkg_path.exists() {
                    return Some(pkg_path.to_string_lossy().to_string());
                }
                // Fallback: look for index.dll, index.so, index.dylib
                for ext in &["dll", "so", "dylib"] {
                    let lib_path = dir.join(format!("index.{}", ext));
                    if lib_path.exists() {
                        return Some(lib_path.to_string_lossy().to_string());
                    }
                }
                // Try just the directory for further processing
                return Some(dir.to_string_lossy().to_string());
            }
        }

        // Fallback: try the path as a file directly
        if let Some(path) = &def.path {
            return Some(path.clone());
        }

        None
    }

    /// Extract metadata from `package.json` if present, else use trait defaults.
    fn extract_metadata(&self, path: &str, def: &PluginDefinition) -> PluginMeta {
        // Try package.json alongside the path
        let pkg_path = if path.ends_with("package.json") {
            Path::new(path).to_path_buf()
        } else {
            Path::new(path).parent().map(|p| p.join("package.json")).unwrap_or_default()
        };

        if pkg_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&pkg_path) {
                if let Ok(pkg) = serde_json::from_str::<serde_json::Value>(&content) {
                    let version = pkg.get("version").and_then(|v| v.as_str()).unwrap_or("1.0.0");
                    let author = pkg.get("author")
                        .and_then(|a| a.as_str())
                        .or_else(|| pkg.get("author").and_then(|a| a.get("name")).and_then(|n| n.as_str()))
                        .unwrap_or("unknown");
                    let topic = pkg.get("homepage")
                        .and_then(|h| h.as_str())
                        .or_else(|| pkg.get("repository").and_then(|r| r.as_str()))
                        .or_else(|| pkg.get("repository").and_then(|r| r.get("url")).and_then(|u| u.as_str()));
                    return PluginMeta {
                        name: def.name.clone(),
                        version: version.to_string(),
                        author: author.to_string(),
                        topic: topic.map(|s| s.to_string()),
                    };
                }
            }
        }

        PluginMeta {
            name: def.name.clone(),
            version: String::new(),
            author: String::new(),
            topic: None,
        }
    }

    /// Load a plugin module from a resolved path.
    /// Returns the plugin box and an optional library handle.
    fn load_module(&self, path: &str) -> Result<(Box<dyn Plugin>, Option<Box<Library>>), Box<dyn std::error::Error>> {
        let ext = Path::new(path).extension().and_then(|s| s.to_str()).unwrap_or("");
        match ext {
            "dll" | "so" | "dylib" | "dylink" => {
                let dp = DynamicPlugin::new(path)?;
                let lib = Box::new(dp._lib);
                return Ok((dp.inner, Some(lib)));
            }
            _ => {}
        }

        Err(format!("Unsupported plugin format: {}", path).into())
    }

    /// Execute a loaded plugin's `init()` with proper context.
    async fn execute_plugin(&self, name: &str, context_type: &PluginContextType, worker_id: &str) {
        let Some(entry) = self.loaded_plugins.get(name) else {
            warn!(target: "Plugins", "Cannot execute plugin '{}': not loaded", name);
            return;
        };

        let ctx = PluginExecutionContext {
            context_type: context_type.clone(),
            worker_id: worker_id.to_string(),
            plugin_name: name.to_string(),
            meta: entry.meta.clone(),
        };

        let config = self.plugin_configs
            .get(name)
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        entry.plugin.init(&ctx, &config).await;
    }

    /// Returns a reference to a plugin by name (for direct access).
    fn get_plugin(&self, name: &str) -> Option<&dyn Plugin> {
        self.loaded_plugins.get(name).map(|e| e.plugin.as_ref())
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Typed dispatch methods — kept for backwards compatibility and ergonomics
// ---------------------------------------------------------------------------

impl PluginManager {
    pub async fn on_player_create(&self, guild_id: &str, session_id: &str, result: &serde_json::Value) {
        for entry in self.loaded_plugins.values() {
            entry.plugin.on_player_create(guild_id, session_id, result).await;
        }
    }

    pub async fn on_player_destroy(&self, guild_id: &str, session_id: &str) {
        for entry in self.loaded_plugins.values() {
            entry.plugin.on_player_destroy(guild_id, session_id).await;
        }
    }

    pub async fn on_track_start(&self, guild_id: &str, track: &serde_json::Value) {
        for entry in self.loaded_plugins.values() {
            entry.plugin.on_track_start(guild_id, track).await;
        }
    }

    pub async fn on_track_end(&self, guild_id: &str, track: &serde_json::Value, reason: &str) {
        for entry in self.loaded_plugins.values() {
            entry.plugin.on_track_end(guild_id, track, reason).await;
        }
    }

    pub async fn on_track_exception(&self, guild_id: &str, track: &serde_json::Value, exception: &serde_json::Value) {
        for entry in self.loaded_plugins.values() {
            entry.plugin.on_track_exception(guild_id, track, exception).await;
        }
    }

    pub async fn on_track_stuck(&self, guild_id: &str, track: &serde_json::Value, threshold_ms: u64) {
        for entry in self.loaded_plugins.values() {
            entry.plugin.on_track_stuck(guild_id, track, threshold_ms).await;
        }
    }

    pub async fn on_search(&self, query: &str, source_name: &str, search_type: &str) {
        for entry in self.loaded_plugins.values() {
            entry.plugin.on_search(query, source_name, search_type).await;
        }
    }

    pub async fn on_resolve(&self, url: &str, source_name: &str) {
        for entry in self.loaded_plugins.values() {
            entry.plugin.on_resolve(url, source_name).await;
        }
    }

    pub async fn on_websocket_connect(&self, session_id: &str) {
        for entry in self.loaded_plugins.values() {
            entry.plugin.on_websocket_connect(session_id).await;
        }
    }

    pub async fn on_websocket_close(&self, session_id: &str, code: u16, reason: &str) {
        for entry in self.loaded_plugins.values() {
            entry.plugin.on_websocket_close(session_id, code, reason).await;
        }
    }

    pub async fn on_websocket_message(&self, session_id: &str, message: &serde_json::Value) {
        for entry in self.loaded_plugins.values() {
            entry.plugin.on_websocket_message(session_id, message).await;
        }
    }

    pub async fn on_player_update(&self, guild_id: &str, state: &serde_json::Value) {
        for entry in self.loaded_plugins.values() {
            entry.plugin.on_player_update(guild_id, state).await;
        }
    }

    pub async fn on_voice_server_update(&self, guild_id: &str, endpoint: &str, token: &str) {
        for entry in self.loaded_plugins.values() {
            entry.plugin.on_voice_server_update(guild_id, endpoint, token).await;
        }
    }

    pub async fn on_audio_packet(&self, guild_id: &str, packet: &[u8]) {
        for entry in self.loaded_plugins.values() {
            entry.plugin.on_audio_packet(guild_id, packet).await;
        }
    }

    pub async fn on_event(&self, guild_id: &str, event: &serde_json::Value) {
        for entry in self.loaded_plugins.values() {
            entry.plugin.on_event(guild_id, event).await;
        }
    }

    pub async fn on_rest_request(&self, method: &str, path: &str, headers: &serde_json::Value) {
        for entry in self.loaded_plugins.values() {
            entry.plugin.on_rest_request(method, path, headers).await;
        }
    }

    pub async fn on_ipc_message(&self, message: &serde_json::Value) {
        for entry in self.loaded_plugins.values() {
            entry.plugin.on_ipc_message(message).await;
        }
    }
}
