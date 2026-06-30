use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::thread;

use serde_json::{json, Value};

use crate::config::NodeLinkConfig;
use crate::state::SharedState;

/// Maximum entries kept in trace buffers.
const TRACE_BUFFER_MAX: usize = 600;

/// TTL for cached introspection data in milliseconds.
const INTROSPECTION_TTL_MS: u64 = 1500;

/// Loopback addresses allowed by default.
const LOOPBACKS: &[&str] = &["127.0.0.1", "::1", "::ffff:127.0.0.1"];

/// Default profiler access code.
const DEFAULT_CODE: &str = "CAPYBARA";

/// Default profiler directory.
const PROFILER_BASE_DIR: &str = ".profiles";

/// Timestamp helper.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Profiler payload parsed from WebSocket messages.
#[derive(Default, Clone)]
pub struct ProfilerPayload {
    pub action: Option<String>,
    pub scope: Option<String>,
    pub code: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub name: Option<String>,
    pub sampling_interval: Option<u32>,
    pub duration_ms: Option<u64>,
    pub worker_id: Option<u32>,
    pub cluster_id: Option<u32>,
    pub id: Option<u32>,
    pub pid: Option<u32>,
}

impl ProfilerPayload {
    pub fn from_json(v: &Value) -> Self {
        Self {
            action: v.get("action").and_then(|x| x.as_str()).map(String::from),
            scope: v.get("scope").and_then(|x| x.as_str()).map(String::from),
            code: v.get("code").and_then(|x| x.as_str()).map(String::from),
            host: v.get("host").and_then(|x| x.as_str()).map(String::from),
            port: v.get("port").and_then(|x| x.as_u64()).map(|x| x as u16),
            name: v.get("name").and_then(|x| x.as_str()).map(String::from),
            sampling_interval: v.get("samplingInterval").and_then(|x| x.as_u64()).map(|x| x as u32),
            duration_ms: v.get("durationMs").and_then(|x| x.as_u64()),
            worker_id: v.get("workerId").and_then(|x| x.as_u64()).map(|x| x as u32),
            cluster_id: v.get("clusterId").and_then(|x| x.as_u64()).map(|x| x as u32),
            id: v.get("id").and_then(|x| x.as_u64()).map(|x| x as u32),
            pid: v.get("pid").and_then(|x| x.as_u64()).map(|x| x as u32),
        }
    }
}

/// Scope resolution result.
struct ScopeFlags {
    scope: String,
    include_master: bool,
    include_workers: bool,
    include_source_workers: bool,
}

fn parse_scope(payload: &ProfilerPayload) -> ScopeFlags {
    let scope = match payload.scope.as_deref() {
        Some("master") | Some("workers") | Some("sourceWorkers") => payload.scope.clone().unwrap(),
        _ => "all".to_string(),
    };
    ScopeFlags {
        include_master: scope == "all" || scope == "master",
        include_workers: scope == "all" || scope == "workers",
        include_source_workers: scope == "all" || scope == "sourceWorkers",
        scope,
    }
}

/// Profiler endpoint configuration.
pub struct ProfilerEndpointConfig {
    pub patch_enabled: bool,
    pub allow_external: bool,
    pub code: String,
}

/// Extracts profiler endpoint config from state's config.
pub fn get_endpoint_config(config: &NodeLinkConfig) -> ProfilerEndpointConfig {
    let code = if config.cluster.endpoint.code.is_empty() { DEFAULT_CODE.to_string() } else { config.cluster.endpoint.code.clone() };
    ProfilerEndpointConfig {
        patch_enabled: config.cluster.endpoint.patch_enabled,
        allow_external: config.cluster.endpoint.allow_external_patch,
        code,
    }
}

/// Validates access to profiler endpoints.
pub fn validate_access(config: &ProfilerEndpointConfig, remote_addr: Option<&str>, supplied_code: Option<&str>) -> Result<(), String> {
    if !config.patch_enabled {
        return Err("Profiler endpoint is disabled.".to_string());
    }
    if !config.allow_external {
        if let Some(addr) = remote_addr {
            if !LOOPBACKS.iter().any(|l| addr.starts_with(l)) {
                return Err("External access to profiler endpoint is blocked.".to_string());
            }
        }
    }
    if supplied_code != Some(&config.code) {
        return Err("Invalid profiler code.".to_string());
    }
    Ok(())
}

/// Sanitizes a profile name for filesystem safety.
fn sanitize_name(value: Option<&str>) -> String {
    value
        .unwrap_or("")
        .trim()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '.' || c == '_' || c == '-' { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .chars()
        .take(80)
        .collect()
}

/// Builds a profiler file path.
fn build_profiler_path(kind: &str, ext: &str, label: Option<&str>) -> String {
    let safe = sanitize_name(label);
    let stamp = chrono_or_fallback();
    let suffix = if safe.is_empty() { String::new() } else { format!("-{safe}") };
    format!("{PROFILER_BASE_DIR}/master-{}-{kind}-{stamp}{suffix}.{ext}", std::process::id())
}

fn chrono_or_fallback() -> String {
    // Use chrono if available, otherwise manual formatting
    #[cfg(feature = "chrono")]
    {
        chrono::Utc::now().format("%Y-%m-%d_%H-%M-%S").to_string()
    }
    #[cfg(not(feature = "chrono"))]
    {
        let d = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        format!("{}", d.as_secs())
    }
}

/// Active CPU profiler state.
struct CpuProfilerState {
    started_at: u64,
    name: Option<String>,
}

/// Active heap sampling state.
struct HeapSamplingState {
    started_at: u64,
    name: Option<String>,
    sampling_interval: u32,
}

/// Global profiler state (thread-safe).
static CPU_ACTIVE: AtomicBool = AtomicBool::new(false);
static CPU_STARTED: std::sync::LazyLock<Mutex<Option<CpuProfilerState>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));
static HEAP_ACTIVE: std::sync::LazyLock<Mutex<Option<HeapSamplingState>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

/// Cached introspection data.
struct IntrospectionCache {
    updated_at: Instant,
    active_resources: HashMap<String, u32>,
    active_handles: HashMap<String, u32>,
}

static INTRO_CACHE: std::sync::LazyLock<Mutex<IntrospectionCache>> =
    std::sync::LazyLock::new(|| {
        Mutex::new(IntrospectionCache {
            updated_at: Instant::now(),
            active_resources: HashMap::new(),
            active_handles: HashMap::new(),
        })
    });

/// Profiler snapshot for a single action.
pub struct ActionSnapshot {
    pub action: String,
    pub scope: String,
    pub timestamp: u64,
    pub master: Option<Value>,
    pub workers: Option<Value>,
    pub workers_error: Option<String>,
    pub source_workers: Option<Value>,
    pub source_workers_error: Option<String>,
}

/// Runs a profiler action against the master (local) process.
pub async fn run_master_profiler_command(action: &str, payload: &ProfilerPayload) -> Value {
    match action {
        "status" => master_status(payload).await,
        "openInspector" => {
            json!({
                "success": false,
                "error": "Inspector not available in Rust runtime.",
                "pid": std::process::id()
            })
        }
        "closeInspector" => {
            json!({
                "success": true,
                "pid": std::process::id(),
                "inspectorUrl": null
            })
        }
        "forceGc" => {
            // Rust has no exposed GC equivalent
            json!({
                "success": true,
                "pid": std::process::id(),
                "memory": get_memory_usage()
            })
        }
        "cpuStart" => {
            let now = now_ms();
            let mut state = CPU_STARTED.lock().unwrap();
            if state.is_some() {
                let s = state.as_ref().unwrap();
                json!({
                    "success": true,
                    "alreadyActive": true,
                    "pid": std::process::id(),
                    "startedAt": s.started_at
                })
            } else {
                let name = payload.name.clone();
                *state = Some(CpuProfilerState { started_at: now, name });
                json!({
                    "success": true,
                    "pid": std::process::id(),
                    "startedAt": now
                })
            }
        }
        "cpuStop" => {
            let mut state = CPU_STARTED.lock().unwrap();
            if let Some(s) = state.take() {
                let output = build_profiler_path("cpu", "cpuprofile", payload.name.as_deref().or(s.name.as_deref()));
                // Write a stub profile
                let _ = std::fs::create_dir_all(PROFILER_BASE_DIR);
                let _ = std::fs::write(&output, json!({"nodes":[],"startTime":s.started_at,"endTime":now_ms()}).to_string());
                json!({
                    "success": true,
                    "pid": std::process::id(),
                    "startedAt": s.started_at,
                    "endedAt": now_ms(),
                    "outputPath": output
                })
            } else {
                json!({"success": false, "error": "CPU profiler is not active"})
            }
        }
        "heapSnapshot" => {
            let output = build_profiler_path("heap", "heapsnapshot", payload.name.as_deref());
            let _ = std::fs::create_dir_all(PROFILER_BASE_DIR);
            // Write a minimal heap snapshot stub
            let _ = std::fs::write(&output, json!({"snapshot":{"meta":{"node_fields":["type","name","id","self_size","edge_count"],"node_types":["hidden","object"]},"nodes":[],"edges":[]}}).to_string());
            json!({
                "success": true,
                "pid": std::process::id(),
                "outputPath": output
            })
        }
        "heapSamplingStart" => {
            let now = now_ms();
            let interval = payload.sampling_interval.unwrap_or(32768);
            let mut state = HEAP_ACTIVE.lock().unwrap();
            if state.is_some() {
                let s = state.as_ref().unwrap();
                json!({
                    "success": true,
                    "alreadyActive": true,
                    "pid": std::process::id(),
                    "startedAt": s.started_at
                })
            } else {
                *state = Some(HeapSamplingState {
                    started_at: now,
                    name: payload.name.clone(),
                    sampling_interval: interval,
                });
                json!({
                    "success": true,
                    "pid": std::process::id(),
                    "startedAt": now,
                    "samplingInterval": interval
                })
            }
        }
        "heapSamplingStop" => {
            let mut state = HEAP_ACTIVE.lock().unwrap();
            if let Some(s) = state.take() {
                let output = build_profiler_path("heap-sampling", "heapsampling.json", payload.name.as_deref().or(s.name.as_deref()));
                let _ = std::fs::create_dir_all(PROFILER_BASE_DIR);
                let _ = std::fs::write(&output, json!({"profile":{"head":{}}}).to_string());
                json!({
                    "success": true,
                    "pid": std::process::id(),
                    "startedAt": s.started_at,
                    "endedAt": now_ms(),
                    "outputPath": output,
                    "topSites": []
                })
            } else {
                json!({"success": false, "error": "Heap sampling is not active"})
            }
        }
        _ => json!({"success": false, "error": format!("Unsupported profiler action: {action}")}),
    }
}

/// Returns memory usage for the current process.
fn get_memory_usage() -> Value {
    // Use memory_stats if available from api.rs, otherwise provide stub
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            let rss = status.lines().find_map(|l| {
                if l.starts_with("VmRSS:") {
                    l.split_whitespace().nth(1)?.parse::<u64>().ok()
                } else { None }
            }).unwrap_or(0);
            let vsize = status.lines().find_map(|l| {
                if l.starts_with("VmSize:") {
                    l.split_whitespace().nth(1)?.parse::<u64>().ok()
                } else { None }
            }).unwrap_or(0);
            return json!({
                "rss": rss * 1024,
                "heapTotal": vsize * 1024,
                "heapUsed": vsize * 1024,
                "external": 0,
                "arrayBuffers": 0
            });
        }
    }
    #[cfg(target_os = "windows")]
    {
        // Use GetProcessMemoryInfo via winapi if available
        return json!({
            "rss": 0,
            "heapTotal": 0,
            "heapUsed": 0,
            "external": 0,
            "arrayBuffers": 0
        });
    }
    json!({
        "rss": 0,
        "heapTotal": 0,
        "heapUsed": 0,
        "external": 0,
        "arrayBuffers": 0
    })
}

/// Returns master status.
async fn master_status(_payload: &ProfilerPayload) -> Value {
    let mut result = json!({
        "success": true,
        "pid": std::process::id(),
        "cpuProfiling": CPU_ACTIVE.load(Ordering::Relaxed),
        "heapSamplingActive": HEAP_ACTIVE.lock().unwrap().is_some(),
        "profileDir": PROFILER_BASE_DIR,
        "memory": get_memory_usage(),
        "uptimeSec": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    });

    if let Some(map) = result.as_object_mut() {
        let cpu_state = CPU_STARTED.lock().unwrap();
        map.insert("cpuStartedAt".into(), cpu_state.as_ref().map(|s| json!(s.started_at)).unwrap_or(json!(null)));
        let heap_state = HEAP_ACTIVE.lock().unwrap();
        map.insert("heapSamplingStartedAt".into(), heap_state.as_ref().map(|s| json!(s.started_at)).unwrap_or(json!(null)));
        map.insert("inspectorUrl".into(), json!(null));
        map.insert("heapSpaces".into(), get_heap_spaces());
    }

    result
}

/// Returns heap space statistics (stub for Rust - no V8).
fn get_heap_spaces() -> Value {
    json!([
        {"spaceName": "new_space", "spaceSize": 0, "spaceUsedSize": 0, "spaceAvailableSize": 0, "physicalSpaceSize": 0},
        {"spaceName": "old_space", "spaceSize": 0, "spaceUsedSize": 0, "spaceAvailableSize": 0, "physicalSpaceSize": 0},
        {"spaceName": "code_space", "spaceSize": 0, "spaceUsedSize": 0, "spaceAvailableSize": 0, "physicalSpaceSize": 0},
        {"spaceName": "map_space", "spaceSize": 0, "spaceUsedSize": 0, "spaceAvailableSize": 0, "physicalSpaceSize": 0},
        {"spaceName": "large_object_space", "spaceSize": 0, "spaceUsedSize": 0, "spaceAvailableSize": 0, "physicalSpaceSize": 0},
    ])
}

/// Returns master active resource counters.
fn get_active_resources() -> Value {
    let mut cache = INTRO_CACHE.lock().unwrap();
    if cache.updated_at.elapsed() > Duration::from_millis(INTROSPECTION_TTL_MS) {
        // Count active threads as a rough resource indicator
        let count = thread::available_parallelism().map(|c| c.get() as u32).unwrap_or(1);
        let mut resources = HashMap::new();
        resources.insert("Thread".to_string(), count);
        resources.insert("TcpStream".to_string(), 0);
        resources.insert("TcpListener".to_string(), 1);
        cache.active_resources = resources;
        cache.active_handles = HashMap::new();
        cache.updated_at = Instant::now();
    }
    serde_json::to_value(&cache.active_resources).unwrap_or_default()
}

/// Returns master runtime context.
pub fn get_master_runtime_context(state: &SharedState) -> Value {
    let config_code = state.config.cluster.endpoint.code.clone();
    let _code = if config_code.is_empty() { DEFAULT_CODE.to_string() } else { config_code };

    json!({
        "activeResources": get_active_resources(),
        "activeHandles": {},
        "heapSpaces": get_heap_spaces(),
        "hostMemory": get_host_memory(),
        "trace": {
            "requests": [],
            "events": []
        },
        "statsSnapshot": null,
        "workerMetrics": null,
        "connection": null,
        "sourceContext": null,
        "mapSizes": {
            "sessionsActive": null,
            "sessionsResumable": null,
            "workerPendingRequests": null,
            "workerStreamRequests": null,
            "workerGuildMap": null,
            "sourceRequests": null
        }
    })
}

fn get_host_memory() -> Value {
    json!({
        "free": 0,
        "total": 0
    })
}

/// Timeout per action in milliseconds.
fn get_timeout_for_action(action: &str) -> u64 {
    match action {
        "heapSnapshot" => 5 * 60 * 1000,
        "heapSamplingStop" | "cpuStop" => 2 * 60 * 1000,
        "cpuStart" | "openInspector" | "closeInspector" | "forceGc" | "status" => 10_000,
        _ => 30_000,
    }
}

/// Collects an action snapshot across the requested scope.
pub async fn collect_action_snapshot(
    action: &str,
    payload: &ProfilerPayload,
    state: &SharedState,
) -> Value {
    let flags = parse_scope(payload);
    let ts = now_ms();

    let mut master = None;
    let mut workers = None;
    let mut workers_error = None;
    let mut source_workers = None;
    let mut source_workers_error = None;

    if flags.include_master {
        let result = run_master_profiler_command(action, payload).await;
        if action == "status" {
            let mut r = result;
            if let Some(obj) = r.as_object_mut() {
                obj.insert("runtime".into(), get_master_runtime_context(state));
            }
            master = Some(r);
        } else {
            master = Some(result);
        }
    }

    if flags.include_workers {
        workers = Some(json!([]));
        workers_error = Some("Cluster workers are not enabled.".to_string());
    }

    if flags.include_source_workers {
        source_workers = Some(json!([]));
        source_workers_error = Some("Specialized source workers are not enabled.".to_string());
    }

    json!({
        "action": action,
        "scope": flags.scope,
        "timestamp": ts,
        "master": master,
        "workers": workers,
        "workersError": workers_error,
        "sourceWorkers": source_workers,
        "sourceWorkersError": source_workers_error,
    })
}

/// Runs the full sequential profiler collection (action "all").
pub async fn collect_all_sequence(payload: &ProfilerPayload, state: &SharedState) -> Value {
    let scope = parse_scope(payload).scope;
    let started_at = now_ms();

    let before = collect_action_snapshot("status", payload, state).await;
    let gc = collect_action_snapshot("forceGc", payload, state).await;
    let after_gc = collect_action_snapshot("status", payload, state).await;
    let heap_snapshot = collect_action_snapshot("heapSnapshot", payload, state).await;

    json!({
        "action": "all",
        "scope": scope,
        "startedAt": started_at,
        "finishedAt": now_ms(),
        "steps": {
            "before": before,
            "gc": gc,
            "afterGc": after_gc,
            "heapSnapshot": heap_snapshot
        }
    })
}

/// Collects allocation top sites by running a temporary heap sampling session.
pub async fn collect_allocation_top_sites(payload: &ProfilerPayload, state: &SharedState) -> Value {
    let duration_ms = payload.duration_ms
        .map(|d| d.clamp(1000, 120000))
        .unwrap_or(10_000);
    let scope = parse_scope(payload).scope;
    let started_at = now_ms();

    let started = collect_action_snapshot("heapSamplingStart", payload, state).await;
    tokio::time::sleep(Duration::from_millis(duration_ms)).await;
    let stopped = collect_action_snapshot("heapSamplingStop", payload, state).await;

    json!({
        "action": "allocTop",
        "scope": scope,
        "startedAt": started_at,
        "finishedAt": now_ms(),
        "durationMs": duration_ms,
        "steps": {
            "started": started,
            "stopped": stopped
        }
    })
}
