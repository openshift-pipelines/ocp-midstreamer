use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Supported performance test scenarios from openshift-pipelines/performance.
#[derive(Debug, Clone, PartialEq)]
pub enum PerfScenario {
    Math,           // Basic math operations (fast, good for testing setup)
    Build,          // Build-related pipelines
    SigningOngoing, // Continuous signing
    ClusterResolver,// Cluster resolver tests
}

impl std::str::FromStr for PerfScenario {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "math" => Ok(PerfScenario::Math),
            "build" => Ok(PerfScenario::Build),
            "signing-ongoing" | "signing_ongoing" => Ok(PerfScenario::SigningOngoing),
            "cluster-resolver" | "cluster_resolver" => Ok(PerfScenario::ClusterResolver),
            _ => Err(format!(
                "Unknown scenario '{}'. Valid: math, build, signing-ongoing, cluster-resolver",
                s
            )),
        }
    }
}

impl PerfScenario {
    pub fn as_env_value(&self) -> &'static str {
        match self {
            PerfScenario::Math => "math",
            PerfScenario::Build => "build",
            PerfScenario::SigningOngoing => "signing-ongoing",
            PerfScenario::ClusterResolver => "cluster-resolver",
        }
    }
}

/// Results from a performance test run.
#[derive(Debug, Serialize, Deserialize)]
pub struct PerfResult {
    pub scenario: String,
    pub passed: bool,
    pub duration_seconds: f64,
    pub metrics: PerfMetrics,
    pub error_message: Option<String>,
}

/// Performance metrics collected during test.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct PerfMetrics {
    /// Total pipeline runs executed
    pub total_runs: Option<u64>,
    /// Successful pipeline runs
    pub successful_runs: Option<u64>,
    /// Failed pipeline runs
    pub failed_runs: Option<u64>,
    /// Average pipeline duration in seconds
    pub avg_duration_seconds: Option<f64>,
    /// P50 latency in seconds
    pub p50_latency_seconds: Option<f64>,
    /// P95 latency in seconds
    pub p95_latency_seconds: Option<f64>,
    /// P99 latency in seconds
    pub p99_latency_seconds: Option<f64>,
    /// Throughput (runs per minute)
    pub throughput_per_minute: Option<f64>,
}

const PERF_REPO: &str = "https://github.com/openshift-pipelines/performance.git";
const PERF_DEFAULT_BRANCH: &str = "main";

/// Clone the performance test repository.
pub fn clone_perf_repo(target_dir: &Path, git_ref: Option<&str>) -> Result<PathBuf> {
    let perf_dir = target_dir.join("performance");

    if perf_dir.exists() {
        println!("  Performance repo already cloned, updating...");
        let status = Command::new("git")
            .args(["fetch", "--all"])
            .current_dir(&perf_dir)
            .status()
            .context("Failed to fetch performance repo updates")?;
        if !status.success() {
            eprintln!("WARNING: git fetch failed, continuing with existing state");
        }
    } else {
        println!("  Cloning performance repo...");
        std::fs::create_dir_all(target_dir)
            .context("Failed to create target directory for performance repo")?;
        let status = Command::new("git")
            .args(["clone", "--depth=1", PERF_REPO, perf_dir.to_str().unwrap()])
            .status()
            .context("Failed to clone performance repo")?;
        if !status.success() {
            anyhow::bail!("git clone failed for performance repo");
        }
    }

    // Checkout specific ref if provided
    let checkout_ref = git_ref.unwrap_or(PERF_DEFAULT_BRANCH);
    let status = Command::new("git")
        .args(["checkout", checkout_ref])
        .current_dir(&perf_dir)
        .status()
        .context("Failed to checkout performance repo ref")?;
    if !status.success() {
        anyhow::bail!("git checkout {} failed", checkout_ref);
    }

    Ok(perf_dir)
}

/// Run performance tests using the ci-scripts from the perf repo.
pub fn run_perf_tests(
    perf_repo_dir: &Path,
    scenario: &PerfScenario,
    output_dir: &Path,
    verbose: bool,
) -> Result<PerfResult> {
    let start = std::time::Instant::now();

    println!("  Running performance scenario: {}", scenario.as_env_value());

    // Ensure output directory exists
    std::fs::create_dir_all(output_dir)
        .context("Failed to create perf output directory")?;

    // Setup cluster (if needed)
    let setup_script = perf_repo_dir.join("ci-scripts/setup-cluster.sh");
    if setup_script.exists() {
        println!("    Running cluster setup...");
        let status = Command::new("bash")
            .arg(&setup_script)
            .env("TEST_SCENARIO", scenario.as_env_value())
            .current_dir(perf_repo_dir)
            .status();

        if let Ok(s) = status {
            if !s.success() {
                eprintln!("WARNING: setup-cluster.sh returned non-zero, continuing anyway");
            }
        }
    }

    // Run the load test
    let load_script = perf_repo_dir.join("ci-scripts/load-test.sh");
    if !load_script.exists() {
        anyhow::bail!("load-test.sh not found in performance repo");
    }

    println!("    Executing load test...");
    let output = Command::new("bash")
        .arg(&load_script)
        .env("TEST_SCENARIO", scenario.as_env_value())
        .env("OUTPUT_DIR", output_dir.to_str().unwrap_or("."))
        .current_dir(perf_repo_dir)
        .output()
        .context("Failed to execute load-test.sh")?;

    let passed = output.status.success();
    let duration = start.elapsed().as_secs_f64();

    if verbose {
        println!("    stdout: {}", String::from_utf8_lossy(&output.stdout));
        println!("    stderr: {}", String::from_utf8_lossy(&output.stderr));
    }

    // Collect results
    let collect_script = perf_repo_dir.join("ci-scripts/collect-results.sh");
    let mut metrics = PerfMetrics::default();

    if collect_script.exists() {
        println!("    Collecting results...");
        let result_output = Command::new("bash")
            .arg(&collect_script)
            .env("TEST_SCENARIO", scenario.as_env_value())
            .env("OUTPUT_DIR", output_dir.to_str().unwrap_or("."))
            .current_dir(perf_repo_dir)
            .output();

        if let Ok(ro) = result_output {
            // Try to parse metrics from output
            let stdout = String::from_utf8_lossy(&ro.stdout);
            metrics = parse_perf_metrics(&stdout);
        }
    }

    // Also check for results.json in output_dir
    let results_file = output_dir.join("results.json");
    if results_file.exists() {
        if let Ok(content) = std::fs::read_to_string(&results_file) {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content) {
                // Extract metrics from JSON if available
                if let Some(m) = parsed.get("metrics") {
                    if let Ok(parsed_metrics) = serde_json::from_value::<PerfMetrics>(m.clone()) {
                        metrics = parsed_metrics;
                    }
                }
            }
        }
    }

    Ok(PerfResult {
        scenario: scenario.as_env_value().to_string(),
        passed,
        duration_seconds: duration,
        metrics,
        error_message: if passed {
            None
        } else {
            Some(String::from_utf8_lossy(&output.stderr).to_string())
        },
    })
}

/// Attempt to parse performance metrics from script output.
fn parse_perf_metrics(output: &str) -> PerfMetrics {
    let mut metrics = PerfMetrics::default();

    // Look for common metric patterns in output
    for line in output.lines() {
        let line_lower = line.to_lowercase();
        if line_lower.contains("total runs:") {
            if let Some(num) = extract_number(line) {
                metrics.total_runs = Some(num as u64);
            }
        }
        if line_lower.contains("successful:") {
            if let Some(num) = extract_number(line) {
                metrics.successful_runs = Some(num as u64);
            }
        }
        if line_lower.contains("failed:") {
            if let Some(num) = extract_number(line) {
                metrics.failed_runs = Some(num as u64);
            }
        }
        if line_lower.contains("avg duration:") || line_lower.contains("average:") {
            if let Some(num) = extract_float(line) {
                metrics.avg_duration_seconds = Some(num);
            }
        }
        if line_lower.contains("p50:") || line_lower.contains("median:") {
            if let Some(num) = extract_float(line) {
                metrics.p50_latency_seconds = Some(num);
            }
        }
        if line_lower.contains("p95:") {
            if let Some(num) = extract_float(line) {
                metrics.p95_latency_seconds = Some(num);
            }
        }
        if line_lower.contains("p99:") {
            if let Some(num) = extract_float(line) {
                metrics.p99_latency_seconds = Some(num);
            }
        }
        if line_lower.contains("throughput:") || line_lower.contains("runs/min:") {
            if let Some(num) = extract_float(line) {
                metrics.throughput_per_minute = Some(num);
            }
        }
    }

    metrics
}

/// Extract a numeric value after a colon in a line (e.g., "P50: 8.2" -> 8.2).
fn extract_number(line: &str) -> Option<f64> {
    // Look for value after colon first (most common format: "Key: Value")
    if let Some(idx) = line.find(':') {
        let value_part = &line[idx + 1..];
        for word in value_part.split_whitespace() {
            let cleaned = word.trim_matches(|c: char| !c.is_ascii_digit() && c != '.' && c != '-');
            if let Ok(n) = cleaned.parse::<f64>() {
                return Some(n);
            }
        }
    }
    // Fallback: find first number in line
    line.split_whitespace()
        .filter_map(|s| s.trim_matches(|c: char| !c.is_ascii_digit() && c != '.' && c != '-').parse::<f64>().ok())
        .next()
}

fn extract_float(line: &str) -> Option<f64> {
    extract_number(line)
}

/// Write performance results to JSON file.
pub fn write_perf_results(results: &PerfResult, output_dir: &Path) -> Result<()> {
    let perf_file = output_dir.join("perf-results.json");
    let content = serde_json::to_string_pretty(results)
        .context("Failed to serialize performance results")?;
    std::fs::write(&perf_file, content)
        .context("Failed to write performance results file")?;
    println!("  Performance results written to: {}", perf_file.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_perf_scenario_from_str() {
        assert_eq!("math".parse::<PerfScenario>().unwrap(), PerfScenario::Math);
        assert_eq!("build".parse::<PerfScenario>().unwrap(), PerfScenario::Build);
        assert_eq!("signing-ongoing".parse::<PerfScenario>().unwrap(), PerfScenario::SigningOngoing);
        assert_eq!("signing_ongoing".parse::<PerfScenario>().unwrap(), PerfScenario::SigningOngoing);
        assert_eq!("cluster-resolver".parse::<PerfScenario>().unwrap(), PerfScenario::ClusterResolver);
        assert_eq!("cluster_resolver".parse::<PerfScenario>().unwrap(), PerfScenario::ClusterResolver);
        assert!("invalid".parse::<PerfScenario>().is_err());
    }

    #[test]
    fn test_perf_scenario_as_env_value() {
        assert_eq!(PerfScenario::Math.as_env_value(), "math");
        assert_eq!(PerfScenario::Build.as_env_value(), "build");
        assert_eq!(PerfScenario::SigningOngoing.as_env_value(), "signing-ongoing");
        assert_eq!(PerfScenario::ClusterResolver.as_env_value(), "cluster-resolver");
    }

    #[test]
    fn test_parse_perf_metrics() {
        let output = r#"
            Total Runs: 100
            Successful: 95
            Failed: 5
            Avg Duration: 10.5
            P50: 8.2
            P95: 15.3
            P99: 22.1
            Throughput: 6.5
        "#;
        let metrics = parse_perf_metrics(output);
        assert_eq!(metrics.total_runs, Some(100));
        assert_eq!(metrics.successful_runs, Some(95));
        assert_eq!(metrics.failed_runs, Some(5));
        assert_eq!(metrics.avg_duration_seconds, Some(10.5));
        assert_eq!(metrics.p50_latency_seconds, Some(8.2));
        assert_eq!(metrics.p95_latency_seconds, Some(15.3));
        assert_eq!(metrics.p99_latency_seconds, Some(22.1));
        assert_eq!(metrics.throughput_per_minute, Some(6.5));
    }

    #[test]
    fn test_perf_result_serde() {
        let result = PerfResult {
            scenario: "math".to_string(),
            passed: true,
            duration_seconds: 120.5,
            metrics: PerfMetrics::default(),
            error_message: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: PerfResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.scenario, "math");
        assert!(deserialized.passed);
    }
}
