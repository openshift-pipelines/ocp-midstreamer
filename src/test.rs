use anyhow::{Context, Result};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;

use crate::exec;
use crate::profile;
use crate::progress;
use crate::results;

/// Verify gauge binary exists and required plugins (go, xml-report) are installed.
fn preflight_check() -> Result<()> {
    which::which("gauge").context(
        "gauge binary not found. Install from https://docs.gauge.org/getting_started/installing-gauge",
    )?;

    let version_result = exec::run_cmd("gauge", &["version"])?;
    if version_result.exit_code != 0 {
        anyhow::bail!("gauge version check failed");
    }

    let plugin_output = version_result.stdout.to_lowercase();

    let mut missing = Vec::new();
    if !plugin_output.contains("go") {
        missing.push("go");
    }
    if !plugin_output.contains("xml-report") {
        missing.push("xml-report");
    }

    if !missing.is_empty() {
        let install_cmds: Vec<String> = missing
            .iter()
            .map(|p| format!("gauge install {p}"))
            .collect();
        anyhow::bail!(
            "Missing gauge plugins: {}. Install with:\n  {}",
            missing.join(", "),
            install_cmds.join("\n  ")
        );
    }

    Ok(())
}

/// Clone the release-tests repository into work_dir/release-tests.
/// Tries --branch first (for branch/tag refs), falls back to clone + checkout (for commit SHAs).
fn clone_release_tests(work_dir: &Path, git_ref: &str) -> Result<PathBuf> {
    let dest = work_dir.join("release-tests");
    let dest_str = dest.to_str().unwrap_or_default();
    let repo_url = "https://github.com/openshift-pipelines/release-tests.git";

    // Try clone with --branch (works for branches and tags)
    let branch_result = exec::run_cmd_unchecked(
        "git",
        &["clone", "--depth", "1", "--branch", git_ref, repo_url, dest_str],
    );

    match branch_result {
        Ok(ref r) if r.exit_code == 0 => return Ok(dest),
        _ => {}
    }

    // Fallback: clone default branch then checkout the ref
    exec::run_cmd("git", &["clone", repo_url, dest_str])
        .context("Failed to clone release-tests repository")?;
    exec::run_cmd("git", &["-C", dest_str, "checkout", git_ref])
        .context(format!("Failed to checkout ref '{git_ref}'"))?;

    Ok(dest)
}

/// Run gauge tests with piped output, teeing to both terminal and log files.
/// When profiler is provided, stdout lines are checked for spec boundary events.
/// Returns exit code.
fn run_gauge_tests(test_dir: &Path, tags: &str, output_dir: &Path, profiler: Option<Arc<profile::MetricsCollector>>) -> Result<i32> {
    let logs_dir = output_dir.join("logs");
    fs::create_dir_all(&logs_dir).context("Failed to create logs directory")?;

    let mut child = Command::new("gauge")
        .args([
            "run",
            "--log-level=debug",
            "--verbose",
            "--tags",
            tags,
            "specs/",
        ])
        .current_dir(test_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to execute gauge")?;

    // Tee stdout: print to terminal, collect, and detect spec boundaries for profiling
    let child_stdout = child.stdout.take().expect("stdout was piped");
    let profiler_clone = profiler.clone();
    let stdout_handle = thread::spawn(move || {
        let reader = BufReader::new(child_stdout);
        let mut collected = String::new();
        // Build a runtime handle for async notify_spec_event calls from this sync thread
        let rt = profiler_clone.as_ref().map(|_| {
            tokio::runtime::Handle::current()
        });
        for line in reader.lines() {
            match line {
                Ok(l) => {
                    println!("{}", l);
                    // Check for spec boundary events when profiling
                    if let (Some(p), Some(handle)) = (&profiler_clone, &rt) {
                        if let Some(event) = profile::detect_spec_boundary(&l) {
                            let p = p.clone();
                            let _ = handle.block_on(p.notify_spec_event(event));
                        }
                    }
                    collected.push_str(&l);
                    collected.push('\n');
                }
                Err(e) => {
                    eprintln!("Error reading stdout: {e}");
                    break;
                }
            }
        }
        collected
    });

    // Tee stderr: print to terminal and collect
    let child_stderr = child.stderr.take().expect("stderr was piped");
    let stderr_handle = thread::spawn(move || {
        let reader = BufReader::new(child_stderr);
        let mut collected = String::new();
        for line in reader.lines() {
            match line {
                Ok(l) => {
                    eprintln!("{}", l);
                    collected.push_str(&l);
                    collected.push('\n');
                }
                Err(e) => {
                    eprintln!("Error reading stderr: {e}");
                    break;
                }
            }
        }
        collected
    });

    let status = child.wait().context("Failed to wait for gauge process")?;

    let stdout_content = stdout_handle.join().unwrap_or_default();
    let stderr_content = stderr_handle.join().unwrap_or_default();

    fs::write(logs_dir.join("test-stdout.log"), &stdout_content)
        .context("Failed to write test-stdout.log")?;
    fs::write(logs_dir.join("test-stderr.log"), &stderr_content)
        .context("Failed to write test-stderr.log")?;

    // If gauge failed, dump its internal logs for diagnostics
    let exit_code = status.code().unwrap_or(-1);
    if exit_code != 0 {
        let gauge_log = test_dir.join("logs").join("gauge.log");
        if gauge_log.exists() {
            if let Ok(content) = fs::read_to_string(&gauge_log) {
                eprintln!("\n=== Gauge internal log ({}) ===", gauge_log.display());
                // Print last 80 lines to avoid flooding
                let lines: Vec<&str> = content.lines().collect();
                let start = lines.len().saturating_sub(80);
                for line in &lines[start..] {
                    eprintln!("{}", line);
                }
                eprintln!("=== End gauge log ===\n");
            }
        }
        // Also check for gauge's Go runner log
        let go_runner_log = test_dir.join("logs").join("gauge-go.log");
        if go_runner_log.exists() {
            if let Ok(content) = fs::read_to_string(&go_runner_log) {
                eprintln!("\n=== Gauge Go runner log ===");
                let lines: Vec<&str> = content.lines().collect();
                let start = lines.len().saturating_sub(40);
                for line in &lines[start..] {
                    eprintln!("{}", line);
                }
                eprintln!("=== End Go runner log ===\n");
            }
        }
    }

    Ok(exit_code)
}

/// Find the JUnit XML report file in gauge's output directory.
fn find_junit_xml(test_dir: &Path) -> Option<PathBuf> {
    let xml_report_dir = test_dir.join("reports").join("xml-report");
    if !xml_report_dir.exists() {
        return None;
    }

    // Look for result.xml first, then any .xml file
    let result_xml = xml_report_dir.join("result.xml");
    if result_xml.exists() {
        return Some(result_xml);
    }

    // Fallback: any .xml file
    if let Ok(entries) = fs::read_dir(&xml_report_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("xml") {
                return Some(path);
            }
        }
    }

    None
}

/// Set up profiling: connect to cluster, check metrics availability, collect capacity and baseline, start collector.
/// Returns None if metrics are unavailable (with warnings printed).
async fn setup_profiler() -> Result<Option<(kube::Client, profile::ClusterCapacity, profile::ResourceSnapshot, profile::MetricsCollector)>> {
    let client = kube::Client::try_default().await
        .context("Could not connect to cluster for profiling")?;

    match profile::check_metrics_available(&client).await? {
        false => {
            eprintln!("Warning: Metrics server not available, skipping profiling");
            return Ok(None);
        }
        true => {}
    }

    let cluster = profile::collect_cluster_capacity(&client).await
        .context("Failed to collect cluster capacity")?;
    let baseline = profile::collect_baseline(&client).await
        .context("Failed to collect baseline")?;
    let collector = profile::MetricsCollector::start(client.clone());

    Ok(Some((client, cluster, baseline, collector)))
}

/// Orchestrate the full test execution flow:
/// 1. Preflight checks (gauge binary + plugins)
/// 2. Clone release-tests repo
/// 3. Run gauge tests with log capture
/// 4. Parse results, print summary, write JSON
///
/// Returns Ok(true) if tests passed, Ok(false) if tests failed.
pub async fn run_tests(tags: &str, release_tests_ref: &str, output_dir: &Path, _verbose: bool, profile: bool) -> Result<bool> {
    // Stage 1: Preflight checks
    let pb = progress::stage_spinner("Preflight checks");
    preflight_check()?;
    progress::finish_spinner(&pb, true);

    // Stage 2: Clone release-tests
    let pb = progress::stage_spinner("Clone release-tests");
    let temp_dir = tempfile::tempdir()?;
    let test_dir = clone_release_tests(temp_dir.path(), release_tests_ref)?;
    progress::finish_spinner(&pb, true);

    // Increase gauge's runner_connection_timeout in GAUGE_HOME config.
    // The default 30s is too short: gauge's Go runner must download+compile all
    // release-tests Go dependencies on first run in the container.
    // Note: the project's env/default/default.properties is NOT used for this setting.
    if let Ok(gauge_home) = std::env::var("GAUGE_HOME").or_else(|_| {
        std::env::var("HOME").map(|h| format!("{}/.gauge", h))
    }) {
        let config_dir = Path::new(&gauge_home).join("config");
        let config_file = config_dir.join("gauge.properties");
        let _ = fs::create_dir_all(&config_dir);
        if config_file.exists() {
            if let Ok(content) = fs::read_to_string(&config_file) {
                if !content.contains("runner_connection_timeout = 3600000") {
                    let updated = content.replace(
                        "runner_connection_timeout",
                        "# runner_connection_timeout"
                    ) + "\nrunner_connection_timeout = 3600000\n";
                    let _ = fs::write(&config_file, updated);
                    eprintln!("Set runner_connection_timeout = 3600000 in {}", config_file.display());
                }
            }
        } else {
            let _ = fs::write(&config_file, "runner_connection_timeout = 3600000\n");
            eprintln!("Created {} with runner_connection_timeout = 3600000", config_file.display());
        }
    }

    // Stage 2.5: Set up profiler if requested
    let mut profiling_ctx: Option<(kube::Client, profile::ClusterCapacity, profile::ResourceSnapshot, Arc<profile::MetricsCollector>)> = None;

    if profile {
        match setup_profiler().await {
            Ok(Some((client, cluster, baseline, collector))) => {
                let arc_collector = Arc::new(collector);
                profiling_ctx = Some((client, cluster, baseline, arc_collector));
            }
            Ok(None) => {} // warnings already printed
            Err(e) => {
                eprintln!("Warning: Profiling setup failed: {e:#}, continuing without profiling");
            }
        }
    }

    // Stage 3: Run gauge tests (streaming with log capture)
    println!("Running Gauge tests with tags: {tags}");
    let profiler_for_gauge = profiling_ctx.as_ref().map(|(_, _, _, c)| c.clone());
    let exit_code = run_gauge_tests(&test_dir, tags, output_dir, profiler_for_gauge)?;

    // Stage 3.5: Finalize profiling if active
    if let Some((_client, cluster, baseline, collector)) = profiling_ctx {
        // Need to unwrap the Arc to call stop (which takes self)
        match Arc::try_unwrap(collector) {
            Ok(c) => {
                match c.stop().await {
                    Ok(specs) => {
                        // Find peak spec for parallelism calculation
                        let peak_cpu = specs.iter().map(|s| s.cpu.p95).max().unwrap_or(0);
                        let peak_mem = specs.iter().map(|s| s.memory.p95).max().unwrap_or(0);
                        let peak_spec_name = specs.iter()
                            .max_by_key(|s| s.cpu.p95)
                            .map(|s| s.spec_name.clone())
                            .unwrap_or_else(|| "none".to_string());

                        let safety_margin = 20;
                        let max_parallel = profile::calculate_max_parallelism(
                            cluster.allocatable_cpu_millicores,
                            baseline.cpu_millicores,
                            peak_cpu,
                            safety_margin,
                        );

                        // Determine limiting resource
                        let avail_cpu = cluster.allocatable_cpu_millicores.saturating_sub(baseline.cpu_millicores);
                        let avail_mem = cluster.allocatable_memory_bytes.saturating_sub(baseline.memory_bytes);
                        let cpu_parallel = if peak_cpu > 0 { avail_cpu * 80 / 100 / peak_cpu } else { u64::MAX };
                        let mem_parallel = if peak_mem > 0 { avail_mem * 80 / 100 / peak_mem } else { u64::MAX };
                        let limiting = if cpu_parallel <= mem_parallel { "cpu" } else { "memory" };

                        let recommendation = profile::ParallelismRecommendation {
                            max_parallel_specs: max_parallel,
                            limiting_resource: limiting.to_string(),
                            safety_margin_percent: safety_margin,
                            reasoning: format!(
                                "Based on {} specs profiled, peak spec '{}' uses {}m CPU / {}Mi memory. \
                                 With {}% safety margin, {} is the limiting resource.",
                                specs.len(), peak_spec_name,
                                peak_cpu, peak_mem / (1024 * 1024),
                                safety_margin, limiting
                            ),
                        };

                        let resource_profile = profile::ResourceProfile {
                            run_timestamp: {
                                let now = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs();
                                format!("{now}")
                            },
                            cluster: cluster.clone(),
                            baseline: baseline.clone(),
                            specs: specs.clone(),
                            recommendation: recommendation.clone(),
                        };

                        // Write resource-profile.json
                        let results_dir = output_dir.join("results");
                        let _ = fs::create_dir_all(&results_dir);
                        let profile_path = results_dir.join("resource-profile.json");
                        match serde_json::to_string_pretty(&resource_profile) {
                            Ok(json) => {
                                if let Err(e) = fs::write(&profile_path, &json) {
                                    eprintln!("Warning: Failed to write resource profile: {e:#}");
                                }
                            }
                            Err(e) => eprintln!("Warning: Failed to serialize resource profile: {e:#}"),
                        }

                        // Print summary
                        println!("\nResource Profile:");
                        println!("  Cluster: {} nodes, {}m CPU, {}Mi memory allocatable",
                            cluster.node_count,
                            cluster.allocatable_cpu_millicores,
                            cluster.allocatable_memory_bytes / (1024 * 1024));
                        println!("  Baseline: {}m CPU, {}Mi memory, {} pods",
                            baseline.cpu_millicores,
                            baseline.memory_bytes / (1024 * 1024),
                            baseline.pod_count);
                        println!("  Specs profiled: {}", specs.len());
                        if !specs.is_empty() {
                            println!("  Peak spec: {} ({}m CPU, {}Mi memory)",
                                peak_spec_name, peak_cpu, peak_mem / (1024 * 1024));
                        }
                        println!("  Recommended parallelism: {} (limited by {})",
                            max_parallel, limiting);
                        println!("  Profile written to: {}", profile_path.display());
                    }
                    Err(e) => eprintln!("Warning: Failed to collect profiling results: {e:#}"),
                }
            }
            Err(_) => eprintln!("Warning: Could not finalize profiler (still in use)"),
        }
    }

    // Stage 4: Parse results and write output
    let results_dir = output_dir.join("results");
    fs::create_dir_all(&results_dir).context("Failed to create results directory")?;

    match find_junit_xml(&test_dir) {
        Some(xml_path) => {
            // Copy junit.xml to output
            let dest_xml = results_dir.join("junit.xml");
            fs::copy(&xml_path, &dest_xml)
                .with_context(|| format!("Failed to copy JUnit XML to {}", dest_xml.display()))?;

            // Parse and display results
            match results::parse_junit_xml(&xml_path) {
                Ok(result) => {
                    let categorized = results::categorize_results(&result);
                    results::print_categorized_results(&categorized);

                    let json_path = results_dir.join("results.json");
                    results::write_categorized_json(&categorized, &json_path)?;

                    println!("Results written to {}", json_path.display());
                    println!("Logs written to {}/logs/", output_dir.display());
                }
                Err(e) => {
                    eprintln!("Warning: Failed to parse JUnit XML: {e:#}");
                    eprintln!("Raw XML copied to {}", dest_xml.display());
                }
            }
        }
        None => {
            // Fallback: parse Gauge stdout log
            let stdout_log = output_dir.join("logs/test-stdout.log");
            if stdout_log.exists() {
                match results::parse_gauge_stdout(&stdout_log) {
                    Ok(result) => {
                        let categorized = results::categorize_results(&result);
                        results::print_categorized_results(&categorized);

                        let json_path = results_dir.join("results.json");
                        results::write_categorized_json(&categorized, &json_path)?;

                        println!("Results written to {}", json_path.display());
                        println!("Logs written to {}/logs/", output_dir.display());
                    }
                    Err(e) => {
                        eprintln!("Warning: Failed to parse Gauge stdout: {e:#}");
                    }
                }
            } else {
                eprintln!(
                    "Warning: No JUnit XML or Gauge stdout log found. No results to parse."
                );
            }
        }
    }

    Ok(exit_code == 0)
}
