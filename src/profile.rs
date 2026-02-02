use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context as _, Result};
use kube::api::{Api, DynamicObject, GroupVersionKind, ApiResource, ListParams};
use kube::Client;
use k8s_openapi::api::core::v1::Node;
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Mutex};
use tokio::task::JoinHandle;

/// Overall resource profile for a test run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceProfile {
    pub run_timestamp: String,
    pub cluster: ClusterCapacity,
    pub baseline: ResourceSnapshot,
    pub specs: Vec<SpecProfile>,
    pub recommendation: ParallelismRecommendation,
}

/// Cluster-level capacity information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterCapacity {
    pub total_cpu_millicores: u64,
    pub total_memory_bytes: u64,
    pub allocatable_cpu_millicores: u64,
    pub allocatable_memory_bytes: u64,
    pub node_count: u32,
}

/// A snapshot of resource usage at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSnapshot {
    pub cpu_millicores: u64,
    pub memory_bytes: u64,
    pub pod_count: u32,
}

/// Resource profile for a single test spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecProfile {
    pub spec_name: String,
    pub duration_seconds: u64,
    pub samples: u32,
    pub cpu: UsageStats,
    pub memory: UsageStats,
    pub peak_pod_count: u32,
}

/// Statistical summary of resource usage samples.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageStats {
    pub min: u64,
    pub max: u64,
    pub avg: u64,
    pub p95: u64,
}

/// Parallelism recommendation based on resource analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelismRecommendation {
    pub max_parallel_specs: u32,
    pub limiting_resource: String,
    pub safety_margin_percent: u32,
    pub reasoning: String,
}

/// Parse a Kubernetes CPU quantity string into millicores.
pub fn parse_cpu_millicores(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(nano) = s.strip_suffix('n') {
        let n: u64 = nano.parse().ok()?;
        return Some(n / 1_000_000);
    }
    if let Some(milli) = s.strip_suffix('m') {
        let m: u64 = milli.parse().ok()?;
        return Some(m);
    }
    // Whole or fractional cores
    let val: f64 = s.parse().ok()?;
    Some((val * 1000.0) as u64)
}

/// Parse a Kubernetes memory quantity string into bytes.
pub fn parse_memory_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Binary suffixes (Ki, Mi, Gi, Ti)
    if let Some(v) = s.strip_suffix("Ti") {
        return Some(v.parse::<u64>().ok()? * 1024 * 1024 * 1024 * 1024);
    }
    if let Some(v) = s.strip_suffix("Gi") {
        return Some(v.parse::<u64>().ok()? * 1024 * 1024 * 1024);
    }
    if let Some(v) = s.strip_suffix("Mi") {
        return Some(v.parse::<u64>().ok()? * 1024 * 1024);
    }
    if let Some(v) = s.strip_suffix("Ki") {
        return Some(v.parse::<u64>().ok()? * 1024);
    }
    // Decimal suffixes (T, G, M, K)
    if let Some(v) = s.strip_suffix('T') {
        return Some(v.parse::<u64>().ok()? * 1_000_000_000_000);
    }
    if let Some(v) = s.strip_suffix('G') {
        return Some(v.parse::<u64>().ok()? * 1_000_000_000);
    }
    if let Some(v) = s.strip_suffix('M') {
        return Some(v.parse::<u64>().ok()? * 1_000_000);
    }
    if let Some(v) = s.strip_suffix('K') {
        return Some(v.parse::<u64>().ok()? * 1_000);
    }
    // Plain bytes
    s.parse::<u64>().ok()
}

/// Compute usage statistics from a slice of samples.
pub fn compute_stats(samples: &[u64]) -> UsageStats {
    if samples.is_empty() {
        return UsageStats { min: 0, max: 0, avg: 0, p95: 0 };
    }
    let mut sorted = samples.to_vec();
    sorted.sort();
    let min = sorted[0];
    let max = sorted[sorted.len() - 1];
    let sum: u64 = sorted.iter().sum();
    let avg = sum / sorted.len() as u64;
    let p95_idx = ((sorted.len() as f64 * 0.95).ceil() as usize).min(sorted.len()) - 1;
    let p95 = sorted[p95_idx];
    UsageStats { min, max, avg, p95 }
}

/// Calculate maximum parallelism given cluster capacity, baseline usage, per-spec peak, and safety margin.
pub fn calculate_max_parallelism(
    allocatable_cpu_millicores: u64,
    baseline_cpu_millicores: u64,
    per_spec_peak_cpu_millicores: u64,
    safety_margin_percent: u32,
) -> u32 {
    let available = allocatable_cpu_millicores.saturating_sub(baseline_cpu_millicores);
    let safe = available * (100 - safety_margin_percent as u64) / 100;
    if per_spec_peak_cpu_millicores == 0 || safe == 0 {
        return 1;
    }
    let max = (safe / per_spec_peak_cpu_millicores) as u32;
    max.max(1)
}

// ---------------------------------------------------------------------------
// Kubernetes metrics API helpers
// ---------------------------------------------------------------------------

fn pod_metrics_api(client: &Client) -> Api<DynamicObject> {
    let gvk = GroupVersionKind::gvk("metrics.k8s.io", "v1beta1", "PodMetrics");
    let ar = ApiResource::from_gvk(&gvk);
    Api::all_with(client.clone(), &ar)
}

/// Check whether the metrics-server PodMetrics API is available on the cluster.
pub async fn check_metrics_available(client: &Client) -> Result<bool> {
    let api = pod_metrics_api(client);
    match api.list(&ListParams::default().limit(1)).await {
        Ok(_) => Ok(true),
        Err(kube::Error::Api(resp)) if resp.code == 404 => {
            eprintln!("Warning: metrics.k8s.io API not available (metrics-server not installed?)");
            Ok(false)
        }
        Err(e) => {
            eprintln!("Warning: metrics API check failed: {e}");
            Ok(false)
        }
    }
}

/// Collect cluster capacity by summing allocatable resources across all nodes.
pub async fn collect_cluster_capacity(client: &Client) -> Result<ClusterCapacity> {
    let nodes: Api<Node> = Api::all(client.clone());
    let list = nodes.list(&ListParams::default()).await.context("Failed to list nodes")?;

    let mut total_cpu: u64 = 0;
    let mut total_mem: u64 = 0;
    let mut alloc_cpu: u64 = 0;
    let mut alloc_mem: u64 = 0;

    for node in &list.items {
        if let Some(status) = &node.status {
            if let Some(cap) = &status.capacity {
                if let Some(cpu) = cap.get("cpu") {
                    total_cpu += parse_cpu_millicores(&cpu.0).unwrap_or(0);
                }
                if let Some(mem) = cap.get("memory") {
                    total_mem += parse_memory_bytes(&mem.0).unwrap_or(0);
                }
            }
            if let Some(alloc) = &status.allocatable {
                if let Some(cpu) = alloc.get("cpu") {
                    alloc_cpu += parse_cpu_millicores(&cpu.0).unwrap_or(0);
                }
                if let Some(mem) = alloc.get("memory") {
                    alloc_mem += parse_memory_bytes(&mem.0).unwrap_or(0);
                }
            }
        }
    }

    Ok(ClusterCapacity {
        total_cpu_millicores: total_cpu,
        total_memory_bytes: total_mem,
        allocatable_cpu_millicores: alloc_cpu,
        allocatable_memory_bytes: alloc_mem,
        node_count: list.items.len() as u32,
    })
}

/// Collect a baseline resource snapshot by summing current PodMetrics across all namespaces.
pub async fn collect_baseline(client: &Client) -> Result<ResourceSnapshot> {
    let api = pod_metrics_api(client);
    let list = api.list(&ListParams::default()).await.context("Failed to list PodMetrics for baseline")?;

    let mut cpu_total: u64 = 0;
    let mut mem_total: u64 = 0;
    let pod_count = list.items.len() as u32;

    for pod in &list.items {
        if let Some(containers) = pod.data.get("containers").and_then(|v| v.as_array()) {
            for container in containers {
                if let Some(usage) = container.get("usage") {
                    if let Some(cpu) = usage.get("cpu").and_then(|v| v.as_str()) {
                        cpu_total += parse_cpu_millicores(cpu).unwrap_or(0);
                    }
                    if let Some(mem) = usage.get("memory").and_then(|v| v.as_str()) {
                        mem_total += parse_memory_bytes(mem).unwrap_or(0);
                    }
                }
            }
        }
    }

    Ok(ResourceSnapshot {
        cpu_millicores: cpu_total,
        memory_bytes: mem_total,
        pod_count,
    })
}

// ---------------------------------------------------------------------------
// Spec boundary detection
// ---------------------------------------------------------------------------

/// Events signaling spec execution boundaries in Gauge stdout.
#[derive(Debug, Clone, PartialEq)]
pub enum SpecEvent {
    SpecStart(String),
    SpecEnd,
}

/// Detect spec boundary events from a Gauge stdout line.
pub fn detect_spec_boundary(line: &str) -> Option<SpecEvent> {
    let trimmed = line.trim();

    // Start patterns
    // Pattern 1: "# Executing specification <path>"
    if let Some(rest) = trimmed.strip_prefix("# Executing specification") {
        let spec = rest.trim().trim_matches('"');
        if !spec.is_empty() {
            return Some(SpecEvent::SpecStart(spec.to_string()));
        }
    }
    // Pattern 2: "## <spec-name>" (Gauge heading for spec)
    if let Some(rest) = trimmed.strip_prefix("## ") {
        let spec = rest.trim();
        if !spec.is_empty() && !spec.contains("Scenario") {
            return Some(SpecEvent::SpecStart(spec.to_string()));
        }
    }
    // Pattern 3: "Executing Spec: <path>"
    if let Some(rest) = trimmed.strip_prefix("Executing Spec:") {
        let spec = rest.trim();
        if !spec.is_empty() {
            return Some(SpecEvent::SpecStart(spec.to_string()));
        }
    }

    // End patterns
    // Pattern 1: "Successfully generated html-report"
    if trimmed.contains("Successfully generated") {
        return Some(SpecEvent::SpecEnd);
    }
    // Pattern 2: "Specifications:" summary line
    if trimmed.starts_with("Specifications:") {
        return Some(SpecEvent::SpecEnd);
    }
    // Pattern 3: "Scenarios:" summary line (end of spec run)
    if trimmed.starts_with("Scenarios:") {
        return Some(SpecEvent::SpecEnd);
    }

    None
}

// ---------------------------------------------------------------------------
// MetricsCollector - background poller
// ---------------------------------------------------------------------------

/// A single metrics sample captured during polling.
#[derive(Debug, Clone)]
pub struct MetricSample {
    pub timestamp: Instant,
    pub spec_name: Option<String>,
    pub total_cpu_millicores: u64,
    pub total_memory_bytes: u64,
    pub pod_count: u32,
}

/// Background metrics collector that polls PodMetrics at a fixed interval.
pub struct MetricsCollector {
    stop_tx: watch::Sender<bool>,
    poll_handle: JoinHandle<()>,
    samples: Arc<Mutex<Vec<MetricSample>>>,
    current_spec: Arc<Mutex<Option<String>>>,
}

impl MetricsCollector {
    /// Start the background metrics poller (polls every 5 seconds).
    pub fn start(client: Client) -> Self {
        let (stop_tx, stop_rx) = watch::channel(false);
        let samples: Arc<Mutex<Vec<MetricSample>>> = Arc::new(Mutex::new(Vec::new()));
        let current_spec: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        let s = samples.clone();
        let cs = current_spec.clone();

        let poll_handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            let mut stop_rx = stop_rx;
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        match collect_poll_sample(&client).await {
                            Ok((cpu, mem, pods)) => {
                                let spec = cs.lock().await.clone();
                                s.lock().await.push(MetricSample {
                                    timestamp: Instant::now(),
                                    spec_name: spec,
                                    total_cpu_millicores: cpu,
                                    total_memory_bytes: mem,
                                    pod_count: pods,
                                });
                            }
                            Err(e) => {
                                eprintln!("Warning: metrics poll failed (will retry): {e}");
                            }
                        }
                    }
                    _ = stop_rx.changed() => {
                        break;
                    }
                }
            }
        });

        MetricsCollector { stop_tx, poll_handle, samples, current_spec }
    }

    /// Notify the collector of a spec boundary event.
    pub async fn notify_spec_event(&self, event: SpecEvent) {
        let mut spec = self.current_spec.lock().await;
        match event {
            SpecEvent::SpecStart(name) => *spec = Some(name),
            SpecEvent::SpecEnd => *spec = None,
        }
    }

    /// Stop polling and return per-spec profiles built from collected samples.
    pub async fn stop(self) -> Result<Vec<SpecProfile>> {
        let _ = self.stop_tx.send(true);
        let _ = self.poll_handle.await;
        let samples = self.samples.lock().await;

        // Group samples by spec_name
        let mut groups: BTreeMap<String, Vec<&MetricSample>> = BTreeMap::new();
        for sample in samples.iter() {
            if let Some(ref name) = sample.spec_name {
                groups.entry(name.clone()).or_default().push(sample);
            }
        }

        let mut profiles = Vec::new();
        for (spec_name, group) in &groups {
            let cpu_vals: Vec<u64> = group.iter().map(|s| s.total_cpu_millicores).collect();
            let mem_vals: Vec<u64> = group.iter().map(|s| s.total_memory_bytes).collect();
            let peak_pods = group.iter().map(|s| s.pod_count).max().unwrap_or(0);

            let duration = if group.len() >= 2 {
                group.last().unwrap().timestamp.duration_since(group.first().unwrap().timestamp).as_secs()
            } else {
                0
            };

            profiles.push(SpecProfile {
                spec_name: spec_name.clone(),
                duration_seconds: duration,
                samples: group.len() as u32,
                cpu: compute_stats(&cpu_vals),
                memory: compute_stats(&mem_vals),
                peak_pod_count: peak_pods,
            });
        }

        Ok(profiles)
    }
}

/// Internal: poll PodMetrics once and sum usage.
async fn collect_poll_sample(client: &Client) -> Result<(u64, u64, u32)> {
    let api = pod_metrics_api(client);
    let list = api.list(&ListParams::default()).await.context("poll PodMetrics")?;
    let mut cpu: u64 = 0;
    let mut mem: u64 = 0;
    let pods = list.items.len() as u32;
    for pod in &list.items {
        if let Some(containers) = pod.data.get("containers").and_then(|v| v.as_array()) {
            for container in containers {
                if let Some(usage) = container.get("usage") {
                    if let Some(c) = usage.get("cpu").and_then(|v| v.as_str()) {
                        cpu += parse_cpu_millicores(c).unwrap_or(0);
                    }
                    if let Some(m) = usage.get("memory").and_then(|v| v.as_str()) {
                        mem += parse_memory_bytes(m).unwrap_or(0);
                    }
                }
            }
        }
    }
    Ok((cpu, mem, pods))
}

#[cfg(test)]
mod tests {
    use super::*;

    // CPU parsing tests
    #[test]
    fn test_parse_cpu_millicore_suffix() {
        assert_eq!(parse_cpu_millicores("100m"), Some(100));
    }

    #[test]
    fn test_parse_cpu_whole_cores() {
        assert_eq!(parse_cpu_millicores("1"), Some(1000));
    }

    #[test]
    fn test_parse_cpu_fractional() {
        assert_eq!(parse_cpu_millicores("0.5"), Some(500));
    }

    #[test]
    fn test_parse_cpu_large_millicore() {
        assert_eq!(parse_cpu_millicores("1500m"), Some(1500));
    }

    #[test]
    fn test_parse_cpu_nanocores() {
        assert_eq!(parse_cpu_millicores("250000000n"), Some(250));
    }

    #[test]
    fn test_parse_cpu_empty() {
        assert_eq!(parse_cpu_millicores(""), None);
    }

    // Memory parsing tests
    #[test]
    fn test_parse_memory_mi() {
        assert_eq!(parse_memory_bytes("128Mi"), Some(134217728));
    }

    #[test]
    fn test_parse_memory_gi() {
        assert_eq!(parse_memory_bytes("1Gi"), Some(1073741824));
    }

    #[test]
    fn test_parse_memory_k() {
        assert_eq!(parse_memory_bytes("256K"), Some(256000));
    }

    #[test]
    fn test_parse_memory_plain_bytes() {
        assert_eq!(parse_memory_bytes("128974848"), Some(128974848));
    }

    #[test]
    fn test_parse_memory_empty() {
        assert_eq!(parse_memory_bytes(""), None);
    }

    // Stats tests
    #[test]
    fn test_compute_stats_nonempty() {
        let stats = compute_stats(&[100, 200, 300]);
        assert_eq!(stats, UsageStats { min: 100, max: 300, avg: 200, p95: 300 });
    }

    #[test]
    fn test_compute_stats_empty() {
        let stats = compute_stats(&[]);
        assert_eq!(stats, UsageStats { min: 0, max: 0, avg: 0, p95: 0 });
    }

    // Parallelism tests
    #[test]
    fn test_parallelism_basic() {
        // available = 8000 - 2000 = 6000, safe = 6000 * 0.8 = 4800, max = 4800 / 1500 = 3
        assert_eq!(calculate_max_parallelism(8000, 2000, 1500, 20), 3);
    }

    #[test]
    fn test_parallelism_minimum_one() {
        // Even if no room, returns at least 1
        assert_eq!(calculate_max_parallelism(2000, 2000, 1500, 20), 1);
    }

    // Serde round-trip test
    #[test]
    fn test_resource_profile_serde_roundtrip() {
        let profile = ResourceProfile {
            run_timestamp: "2026-02-01T00:00:00Z".to_string(),
            cluster: ClusterCapacity {
                total_cpu_millicores: 16000,
                total_memory_bytes: 68719476736,
                allocatable_cpu_millicores: 14000,
                allocatable_memory_bytes: 60129542144,
                node_count: 4,
            },
            baseline: ResourceSnapshot {
                cpu_millicores: 2000,
                memory_bytes: 4294967296,
                pod_count: 50,
            },
            specs: vec![],
            recommendation: ParallelismRecommendation {
                max_parallel_specs: 3,
                limiting_resource: "cpu".to_string(),
                safety_margin_percent: 20,
                reasoning: "test".to_string(),
            },
        };
        let json = serde_json::to_string(&profile).unwrap();
        let deserialized: ResourceProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.run_timestamp, profile.run_timestamp);
        assert_eq!(deserialized.cluster.node_count, 4);
        assert_eq!(deserialized.recommendation.max_parallel_specs, 3);
    }

    // Spec boundary detection tests
    #[test]
    fn test_detect_spec_start_executing() {
        assert_eq!(
            detect_spec_boundary("# Executing specification specs/pipeline_test.spec"),
            Some(SpecEvent::SpecStart("specs/pipeline_test.spec".to_string()))
        );
    }

    #[test]
    fn test_detect_spec_start_heading() {
        assert_eq!(
            detect_spec_boundary("## Pipeline E2E Tests"),
            Some(SpecEvent::SpecStart("Pipeline E2E Tests".to_string()))
        );
    }

    #[test]
    fn test_detect_spec_start_executing_spec() {
        assert_eq!(
            detect_spec_boundary("Executing Spec: specs/triggers.spec"),
            Some(SpecEvent::SpecStart("specs/triggers.spec".to_string()))
        );
    }

    #[test]
    fn test_detect_spec_end_generated() {
        assert_eq!(
            detect_spec_boundary("Successfully generated html-report to => /tmp/reports"),
            Some(SpecEvent::SpecEnd)
        );
    }

    #[test]
    fn test_detect_spec_end_specifications() {
        assert_eq!(
            detect_spec_boundary("Specifications: 3 executed, 2 passed, 1 failed"),
            Some(SpecEvent::SpecEnd)
        );
    }

    #[test]
    fn test_detect_spec_end_scenarios() {
        assert_eq!(
            detect_spec_boundary("Scenarios: 10 executed, 8 passed, 2 failed"),
            Some(SpecEvent::SpecEnd)
        );
    }

    #[test]
    fn test_detect_spec_boundary_none() {
        assert_eq!(detect_spec_boundary("some random output line"), None);
    }
}
