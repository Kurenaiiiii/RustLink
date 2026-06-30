use std::net::ToSocketAddrs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use serde::Serialize;
use serde_json::json;
use tokio::sync::watch;
use tokio::time::Instant;
use tracing::{debug, info};

/// Connection status classification.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ConnectionStatus {
    Unknown,
    Good,
    Average,
    Bad,
    Disconnected,
}

impl Default for ConnectionStatus {
    fn default() -> Self {
        Self::Unknown
    }
}

/// A test endpoint for speed measurements.
#[derive(Debug, Clone)]
pub struct ConnectionEndpoint {
    pub name: String,
    pub url: String,
    pub expected_size_bytes: u64,
}

/// Result of a DNS connectivity test.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectivityTestResult {
    pub is_online: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Result of a ping test.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PingResult {
    pub host: String,
    pub alive: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub packet_loss: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Network interface information.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInfo {
    pub is_connected: bool,
    pub connection_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wifi_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_servers: Option<Vec<String>>,
}

/// Speed test result.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedTestResult {
    pub bps: f64,
    pub kbps: f64,
    pub mbps: f64,
}

/// Connection metrics snapshot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionMetrics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<SpeedTestResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloaded_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns: Option<ConnectivityTestResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ping: Option<PingResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub timestamp: u64,
}

/// Default test endpoints matching NodeLink.
fn default_endpoints() -> Vec<ConnectionEndpoint> {
    vec![
        ConnectionEndpoint {
            name: "Cachefly".into(),
            url: "http://cachefly.cachefly.net/10mb.test".into(),
            expected_size_bytes: 10 * 1024 * 1024,
        },
        ConnectionEndpoint {
            name: "Cloudflare".into(),
            url: "https://speed.cloudflare.com/__down?bytes=10485760".into(),
            expected_size_bytes: 10 * 1024 * 1024,
        },
        ConnectionEndpoint {
            name: "ThinkBroadband".into(),
            url: "http://ipv4.download.thinkbroadband.com/10MB.zip".into(),
            expected_size_bytes: 10 * 1024 * 1024,
        },
        ConnectionEndpoint {
            name: "Speedtest (Otenet)".into(),
            url: "http://speedtest.ftp.otenet.gr/files/test10Mb.db".into(),
            expected_size_bytes: 10 * 1024 * 1024,
        },
        ConnectionEndpoint {
            name: "Proof".into(),
            url: "http://proof.ovh.net/files/10Mb.dat".into(),
            expected_size_bytes: 10 * 1024 * 1024,
        },
    ]
}

fn default_dns_hosts() -> Vec<String> {
    vec![
        "google.com".into(),
        "cloudflare.com".into(),
        "8.8.8.8".into(),
    ]
}

fn default_ping_hosts() -> Vec<String> {
    vec![
        "1.1.1.1".into(),
        "8.8.8.8".into(),
        "cloudflare.com".into(),
    ]
}

/// Shared connection manager state.
pub struct ConnectionManager {
    running: Arc<AtomicBool>,
    status: Arc<Mutex<ConnectionStatus>>,
    metrics: Arc<Mutex<ConnectionMetrics>>,
    status_tx: watch::Sender<ConnectionStatus>,
    metrics_tx: watch::Sender<ConnectionMetrics>,
    sessions: Arc<Mutex<Vec<String>>>,
    check_interval_ms: u64,
    max_test_duration_ms: u64,
    max_download_bytes: u64,
    endpoints: Vec<ConnectionEndpoint>,
    dns_hosts: Vec<String>,
    ping_hosts: Vec<String>,
    thresholds_bad: f64,
    thresholds_average: f64,
    log_all_checks: bool,
    task_handle: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        let (status_tx, _) = watch::channel(ConnectionStatus::Unknown);
        let (metrics_tx, _) = watch::channel(ConnectionMetrics {
            speed: None,
            downloaded_bytes: None,
            duration_seconds: None,
            latency_ms: None,
            endpoint: None,
            dns: None,
            ping: None,
            network: None,
            error: None,
            timestamp: 0,
        });

        Self {
            running: Arc::new(AtomicBool::new(false)),
            status: Arc::new(Mutex::new(ConnectionStatus::Unknown)),
            metrics: Arc::new(Mutex::new(ConnectionMetrics {
                speed: None,
                downloaded_bytes: None,
                duration_seconds: None,
                latency_ms: None,
                endpoint: None,
                dns: None,
                ping: None,
                network: None,
                error: None,
                timestamp: 0,
            })),
            status_tx,
            metrics_tx,
            sessions: Arc::new(Mutex::new(Vec::new())),
            check_interval_ms: 300_000,
            max_test_duration_ms: 10_000,
            max_download_bytes: 10 * 1024 * 1024,
            endpoints: default_endpoints(),
            dns_hosts: default_dns_hosts(),
            ping_hosts: default_ping_hosts(),
            thresholds_bad: 1.0,
            thresholds_average: 5.0,
            log_all_checks: false,
            task_handle: Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_config(mut self, config: &crate::config::ConnectionConfig) -> Self {
        self.check_interval_ms = config.interval.max(1);
        self.max_test_duration_ms = config.timeout;
        self.log_all_checks = config.log_all_checks;
        self
    }

    pub fn current_status(&self) -> ConnectionStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn current_metrics(&self) -> ConnectionMetrics {
        self.metrics.lock().unwrap().clone()
    }

    pub fn watch_status(&self) -> watch::Receiver<ConnectionStatus> {
        self.status_tx.subscribe()
    }

    pub fn watch_metrics(&self) -> watch::Receiver<ConnectionMetrics> {
        self.metrics_tx.subscribe()
    }

    pub fn register_session(&self, session_id: String) {
        let mut list = self.sessions.lock().unwrap();
        if !list.contains(&session_id) {
            list.push(session_id);
        }
    }

    pub fn unregister_session(&self, session_id: &str) {
        let mut list = self.sessions.lock().unwrap();
        list.retain(|s| s != session_id);
    }

    /// Starts periodic connection checks.
    pub fn start(self: &Arc<Self>) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }

        let this = self.clone();
        let handle = tokio::spawn(async move {
            info!(target: "ConnectionManager", "Starting connection checks every {}ms.", this.check_interval_ms);

            // Run initial check
            this.clone().check_connection().await;

            loop {
                tokio::time::sleep(Duration::from_millis(this.check_interval_ms)).await;
                if !this.running.load(Ordering::SeqCst) {
                    break;
                }
                this.clone().check_connection().await;
            }
        });

        *self.task_handle.lock().unwrap() = Some(handle);
    }

    /// Stops periodic checks.
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.task_handle.lock().unwrap().take() {
            handle.abort();
        }
    }

    /// Runs a full connectivity check.
    pub async fn check_connection(self: Arc<Self>) {
        let now = chrono_now();
        let dns_result = self._test_dns_connectivity().await;
        let ping_result = self._test_ping().await;
        let network_info = self._get_network_info().await;

        // Run speed test on first check or if ping changed significantly
        let should_run_speed = {
            let metrics = self.metrics.lock().unwrap();
            metrics.speed.is_none()
                || (now - metrics.timestamp) > 1_500_000
        };

        if should_run_speed {
            for endpoint in &self.endpoints {
                match self._run_speed_test(endpoint).await {
                    Some(result) => {
                        let speed_mbps = result.speed_mbps;
                        let new_status = self._classify_status(speed_mbps);

                        let metrics = ConnectionMetrics {
                            speed: Some(SpeedTestResult {
                                bps: result.speed_bps,
                                kbps: result.speed_kbps,
                                mbps: result.speed_mbps,
                            }),
                            downloaded_bytes: Some(result.downloaded_bytes),
                            duration_seconds: Some(result.duration_seconds),
                            latency_ms: result.latency_ms,
                            endpoint: Some(json!({
                                "name": endpoint.name,
                                "url": endpoint.url,
                            })),
                            dns: dns_result.clone(),
                            ping: ping_result.clone(),
                            network: network_info.clone(),
                            error: None,
                            timestamp: now,
                        };

                        *self.metrics.lock().unwrap() = metrics.clone();
                        self._update_status(new_status).await;
                        self.broadcast_status(&metrics).await;
                        return;
                    }
                    None => continue,
                }
            }

            // All endpoints failed
            let metrics = ConnectionMetrics {
                speed: None,
                downloaded_bytes: None,
                duration_seconds: None,
                latency_ms: None,
                endpoint: None,
                dns: dns_result.clone(),
                ping: ping_result.clone(),
                network: network_info.clone(),
                error: Some("All connection tests failed".into()),
                timestamp: now,
            };
            *self.metrics.lock().unwrap() = metrics.clone();
            self._update_status(ConnectionStatus::Disconnected).await;
            self.broadcast_status(&metrics).await;
        } else {
            // No speed test needed, just update DNS/ping/network
            let snapshot = {
                let mut metrics = self.metrics.lock().unwrap();
                metrics.dns = dns_result;
                metrics.ping = ping_result;
                metrics.network = network_info;
                metrics.timestamp = now;
                metrics.clone()
            };
            self.broadcast_status(&snapshot).await;
        }
    }

    async fn _update_status(&self, new_status: ConnectionStatus) {
        let old = {
            let mut s = self.status.lock().unwrap();
            let old = s.clone();
            *s = new_status.clone();
            old
        };

        if new_status != old || self.log_all_checks {
            let _ = self.status_tx.send(new_status);
        }
    }

    async fn broadcast_status(&self, metrics: &ConnectionMetrics) {
        let _ = self.metrics_tx.send(metrics.clone());
    }

    fn _classify_status(&self, speed_mbps: f64) -> ConnectionStatus {
        if speed_mbps < self.thresholds_bad {
            ConnectionStatus::Bad
        } else if speed_mbps < self.thresholds_average {
            ConnectionStatus::Average
        } else {
            ConnectionStatus::Good
        }
    }

    async fn _run_speed_test(
        &self,
        endpoint: &ConnectionEndpoint,
    ) -> Option<SpeedTestIntermediate> {
        let start = Instant::now();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(self.max_test_duration_ms))
            .build()
            .ok()?;

        let response = client
            .get(&endpoint.url)
            .header("Accept-Encoding", "identity")
            .send()
            .await
            .ok()?;

        if response.status() != 200 {
            return None;
        }

        let max_bytes = self.max_download_bytes.max(endpoint.expected_size_bytes);
        let mut downloaded: u64 = 0;
        let mut latency_ms: Option<f64> = None;
        let mut stream = response.bytes_stream();

        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.ok()?;
            if latency_ms.is_none() {
                latency_ms = Some(start.elapsed().as_secs_f64() * 1000.0);
            }
            downloaded += chunk.len() as u64;
            if downloaded >= max_bytes {
                break;
            }
        }

        let duration = start.elapsed().as_secs_f64();
        if duration <= 0.0 || downloaded == 0 {
            return None;
        }

        let speed_bps = downloaded as f64 / duration;
        let speed_kbps = (speed_bps * 8.0) / 1024.0;
        let speed_mbps = speed_kbps / 1024.0;

        Some(SpeedTestIntermediate {
            speed_bps,
            speed_kbps,
            speed_mbps,
            downloaded_bytes: downloaded,
            duration_seconds: duration,
            latency_ms,
        })
    }

    async fn _test_dns_connectivity(&self) -> Option<ConnectivityTestResult> {
        let hosts = if self.dns_hosts.is_empty() {
            default_dns_hosts()
        } else {
            self.dns_hosts.clone()
        };

        for host in &hosts {
            let start = Instant::now();
            let addr = format!("{}:80", host);
            let resolved: Vec<_> = match addr.to_socket_addrs() {
                Ok(addrs) => addrs.collect(),
                Err(_) => Vec::new(),
            };
            if !resolved.is_empty() {
                return Some(ConnectivityTestResult {
                    is_online: true,
                    host: Some(host.clone()),
                    latency_ms: Some(start.elapsed().as_secs_f64() * 1000.0),
                    error: None,
                });
            }
            debug!(target: "ConnectionManager", "DNS check failed for {}", host);
        }

        Some(ConnectivityTestResult {
            is_online: false,
            host: None,
            latency_ms: None,
            error: Some("No DNS host responded".into()),
        })
    }

    async fn _test_ping(&self) -> Option<PingResult> {
        let hosts = if self.ping_hosts.is_empty() {
            default_ping_hosts()
        } else {
            self.ping_hosts.clone()
        };

        for host in &hosts {
            let result = self._ping_host(host).await;
            if result.alive {
                return Some(result);
            }
        }
        None
    }

    async fn _ping_host(&self, host: &str) -> PingResult {
        let os = std::env::consts::OS;
        let (cmd, args) = if os == "windows" {
            ("ping", vec!["-n", "4", "-w", "5000", host])
        } else {
            ("ping", vec!["-c", "4", "-W", "5", host])
        };

        let output = tokio::process::Command::new(cmd)
            .args(&args)
            .output()
            .await;

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);

                if !stderr.trim().is_empty() {
                    return PingResult {
                        host: host.to_string(),
                        alive: false,
                        min_ms: None,
                        avg_ms: None,
                        max_ms: None,
                        packet_loss: None,
                        error: Some(stderr.trim().to_string()),
                    };
                }

                if os == "windows" {
                    Self::_parse_windows_ping(host, &stdout)
                } else {
                    Self::_parse_unix_ping(host, &stdout)
                }
            }
            Err(e) => PingResult {
                host: host.to_string(),
                alive: false,
                min_ms: None,
                avg_ms: None,
                max_ms: None,
                packet_loss: None,
                error: Some(e.to_string()),
            },
        }
    }

    fn _parse_windows_ping(host: &str, output: &str) -> PingResult {
        let mut result = PingResult {
            host: host.to_string(),
            alive: false,
            min_ms: None,
            avg_ms: None,
            max_ms: None,
            packet_loss: None,
            error: None,
        };

        if output.contains("Request timed out") || output.contains("Destination host unreachable") {
            return result;
        }

        if let Some(caps) = regex_lite::Regex::new(r"Minimum = (\d+)ms, Maximum = (\d+)ms, Average = (\d+)ms")
            .ok()
            .and_then(|re| re.captures(output))
        {
            result.alive = true;
            result.min_ms = caps.get(1).and_then(|m| m.as_str().parse().ok());
            result.max_ms = caps.get(2).and_then(|m| m.as_str().parse().ok());
            result.avg_ms = caps.get(3).and_then(|m| m.as_str().parse().ok());
        }

        if let Some(caps) = regex_lite::Regex::new(r"(\d+)% loss")
            .ok()
            .and_then(|re| re.captures(output))
        {
            result.packet_loss = caps.get(1).and_then(|m| m.as_str().parse().ok());
        }

        result
    }

    fn _parse_unix_ping(host: &str, output: &str) -> PingResult {
        let mut result = PingResult {
            host: host.to_string(),
            alive: false,
            min_ms: None,
            avg_ms: None,
            max_ms: None,
            packet_loss: None,
            error: None,
        };

        if output.contains("100% packet loss") || output.contains("Network is unreachable") {
            return result;
        }

        // Try parsing rtt min/avg/max/mdev format
        let re = regex_lite::Regex::new(
            r"(?:round-trip|rtt) min/avg/max/(?:stddev|mdev) = ([\d.]+)/([\d.]+)/([\d.]+)/([\d.]+) ms",
        );
        if let Some(re) = re.ok() {
            if let Some(caps) = re.captures(output) {
                result.alive = true;
                result.min_ms = caps.get(1).and_then(|m| m.as_str().parse().ok());
                result.avg_ms = caps.get(2).and_then(|m| m.as_str().parse().ok());
                result.max_ms = caps.get(3).and_then(|m| m.as_str().parse().ok());
                // packet_loss from unix format? check below
                if let Some(caps) = regex_lite::Regex::new(r"(\d+)% packet loss")
                    .ok()
                    .and_then(|re| re.captures(output))
                {
                    result.packet_loss = caps.get(1).and_then(|m| m.as_str().parse().ok());
                }
                return result;
            }
        }

        // Fallback: parse individual time= entries
        let time_re = regex_lite::Regex::new(r"time=([\d.]+) ms").ok();
        if let Some(re) = time_re {
            let times: Vec<f64> = re
                .captures_iter(output)
                .filter_map(|caps| caps.get(1))
                .filter_map(|m| m.as_str().parse().ok())
                .filter(|t: &f64| *t > 0.0)
                .collect();

            if !times.is_empty() {
                result.alive = true;
                result.min_ms = Some(times.iter().cloned().fold(f64::MAX, f64::min));
                result.max_ms = Some(times.iter().cloned().fold(f64::MIN, f64::max));
                result.avg_ms = Some(times.iter().sum::<f64>() / times.len() as f64);
            }
        }

        if let Some(caps) = regex_lite::Regex::new(r"(\d+)% packet loss")
            .ok()
            .and_then(|re| re.captures(output))
        {
            result.packet_loss = caps.get(1).and_then(|m| m.as_str().parse().ok());
        }

        result
    }

    async fn _get_network_info(&self) -> Option<NetworkInfo> {
        // Get network interfaces via sysinfo or platform-specific commands
        let os = std::env::consts::OS;

        if os == "windows" {
            self._get_windows_network_info().await
        } else if os == "linux" {
            self._get_linux_network_info().await
        } else if os == "macos" {
            self._get_macos_network_info().await
        } else {
            // Fallback: try ipconfig/ifconfig
            self._get_fallback_network_info().await
        }
    }

    async fn _get_windows_network_info(&self) -> Option<NetworkInfo> {
        let output = tokio::process::Command::new("ipconfig")
            .output()
            .await
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        // Extract first non-loopback IPv4 address
        let ip_re = regex_lite::Regex::new(r"IPv4 Address[.\s]*:\s*([0-9.]+)").ok()?;
        let adapter_re = regex_lite::Regex::new(r"Ethernet adapter (.+?):").ok()?;

        let ip_address = ip_re.captures(&stdout).and_then(|c| {
            c.get(1).map(|m| m.as_str().to_string())
        });

        let interface_name = adapter_re.captures(&stdout).and_then(|c| {
            c.get(1).map(|m| m.as_str().to_string())
        });

        let gateway = self._get_gateway_windows().await;

        // Get DNS servers
        let dns_servers = self._get_dns_servers().await;

        if let Some(ip) = ip_address {
            let connection_type = interface_name
                .as_deref()
                .map(|name| {
                    let lower = name.to_lowercase();
                    if lower.contains("wi-fi") || lower.contains("wlan") || lower.starts_with("wl") {
                        "wifi"
                    } else if lower.contains("eth") || lower.starts_with("en") {
                        "ethernet"
                    } else {
                        "unknown"
                    }
                })
                .unwrap_or("unknown")
                .to_string();

            Some(NetworkInfo {
                is_connected: true,
                connection_type,
                ip_address: Some(ip),
                interface_name,
                wifi_name: None,
                gateway,
                dns_servers,
            })
        } else {
            Some(NetworkInfo {
                is_connected: false,
                connection_type: "unknown".into(),
                ip_address: None,
                interface_name: None,
                wifi_name: None,
                gateway: None,
                dns_servers,
            })
        }
    }

    async fn _get_linux_network_info(&self) -> Option<NetworkInfo> {
        // Try `ip addr` first, fallback to `ifconfig`
        let output = tokio::process::Command::new("ip")
            .args(["-4", "addr", "show", "scope", "global"])
            .output()
            .await
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        // Parse first non-loopback interface
        let re = regex_lite::Regex::new(
            r"(\d+):\s+(\w+)[^:]*:\s+.*\n\s+inet\s+([0-9.]+)",
        )
        .ok()?;

        if let Some(caps) = re.captures(&stdout) {
            let interface_name = caps.get(2).map(|m| m.as_str().to_string());
            let ip_address = caps.get(3).map(|m| m.as_str().to_string());
            let gateway = self._get_gateway_linux().await;
            let dns_servers = self._get_dns_servers().await;
            let lower = interface_name.as_deref().unwrap_or("").to_lowercase();

            let connection_type = if lower.contains("wifi") || lower.contains("wlan") || lower.starts_with("wl") {
                "wifi"
            } else if lower.contains("eth") || lower.starts_with("en") {
                "ethernet"
            } else if lower.contains("rmnet") || lower.contains("wwan") {
                "mobile"
            } else {
                "unknown"
            };

            Some(NetworkInfo {
                is_connected: true,
                connection_type: connection_type.to_string(),
                ip_address,
                interface_name,
                wifi_name: None,
                gateway,
                dns_servers,
            })
        } else {
            self._get_fallback_network_info().await
        }
    }

    async fn _get_macos_network_info(&self) -> Option<NetworkInfo> {
        let output = tokio::process::Command::new("ifconfig")
            .output()
            .await
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);

        let re = regex_lite::Regex::new(
            r"(\w+):\s+.*\n(\s+inet\s+([0-9.]+))?",
        )
        .ok()?;

        // Find first non-loopback IPv4 interface
        for caps in re.captures_iter(&stdout) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            if name == "lo0" || name == "lo" {
                continue;
            }
            if let Some(ip) = caps.get(3).map(|m| m.as_str()) {
                let lower = name.to_lowercase();
                let connection_type = if lower.contains("en") || lower.contains("eth") {
                    if lower.starts_with("en0") || lower.starts_with("en1") {
                        // en0 is typically Wi-Fi on Mac, en1 is ethernet
                        // But this is not always the case
                        "ethernet"
                    } else {
                        "unknown"
                    }
                } else if lower.contains("awdl") || lower.contains("llw") {
                    continue; // skip Apple Wireless Direct Link
                } else {
                    "unknown"
                };

                let gateway = self._get_gateway_macos().await;
                let dns_servers = self._get_dns_servers().await;

                return Some(NetworkInfo {
                    is_connected: true,
                    connection_type: connection_type.to_string(),
                    ip_address: Some(ip.to_string()),
                    interface_name: Some(name.to_string()),
                    wifi_name: None,
                    gateway,
                    dns_servers,
                });
            }
        }

        self._get_fallback_network_info().await
    }

    async fn _get_fallback_network_info(&self) -> Option<NetworkInfo> {
        // Cross-platform fallback using DNS servers and interface detection
        let dns_servers = self._get_dns_servers().await;
        Some(NetworkInfo {
            is_connected: !dns_servers.as_ref().map_or(true, |v| v.is_empty()),
            connection_type: "unknown".into(),
            ip_address: None,
            interface_name: None,
            wifi_name: None,
            gateway: None,
            dns_servers,
        })
    }

    async fn _get_dns_servers(&self) -> Option<Vec<String>> {
        let os = std::env::consts::OS;
        if os == "windows" {
            let output = tokio::process::Command::new("ipconfig")
                .args(["/all"])
                .output()
                .await
                .ok()?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let re = regex_lite::Regex::new(r"DNS Servers[.\s]*:\s*([0-9.]+)").ok()?;
            let servers: Vec<String> = re
                .captures_iter(&stdout)
                .filter_map(|caps| caps.get(1))
                .map(|m| m.as_str().to_string())
                .collect();
            if servers.is_empty() {
                None
            } else {
                Some(servers)
            }
        } else if os == "linux" {
            let content = tokio::fs::read_to_string("/etc/resolv.conf")
                .await
                .ok()?;
            let re = regex_lite::Regex::new(r"^\s*nameserver\s+([0-9.]+)").ok()?;
            let servers: Vec<String> = content
                .lines()
                .filter_map(|line| re.captures(line))
                .filter_map(|caps| caps.get(1))
                .map(|m| m.as_str().to_string())
                .collect();
            if servers.is_empty() {
                None
            } else {
                Some(servers)
            }
        } else {
            None
        }
    }

    async fn _get_gateway_windows(&self) -> Option<String> {
        let output = tokio::process::Command::new("ipconfig")
            .output()
            .await
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let re = regex_lite::Regex::new(r"Default Gateway[.\s]*:\s*([0-9.]+)").ok()?;
        re.captures(&stdout)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
    }

    async fn _get_gateway_linux(&self) -> Option<String> {
        let output = tokio::process::Command::new("ip")
            .args(["route"])
            .output()
            .await
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let re = regex_lite::Regex::new(r"default via ([0-9.]+)").ok()?;
        re.captures(&stdout)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
    }

    async fn _get_gateway_macos(&self) -> Option<String> {
        let output = tokio::process::Command::new("route")
            .args(["-n", "get", "default"])
            .output()
            .await
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let re = regex_lite::Regex::new(r"gateway:\s*([0-9.]+)").ok()?;
        re.captures(&stdout)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string())
    }
}

struct SpeedTestIntermediate {
    speed_bps: f64,
    speed_kbps: f64,
    speed_mbps: f64,
    downloaded_bytes: u64,
    duration_seconds: f64,
    latency_ms: Option<f64>,
}

fn chrono_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl Default for ConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}
