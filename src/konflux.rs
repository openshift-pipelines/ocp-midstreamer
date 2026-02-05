use anyhow::{Context, Result};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::exec;
use crate::results;

/// Result of a completed PipelineRun.
#[derive(Debug)]
pub struct PipelineRunResult {
    pub name: String,
    pub status: PipelineRunStatus,
    pub duration: Duration,
    pub reason: String,
}

#[derive(Debug, PartialEq)]
pub enum PipelineRunStatus {
    Succeeded,
    Failed,
    Timeout,
}

/// EaaS-related task names that should be removed for standalone execution.
const EAAS_TASKS: &[&str] = &[
    "provision-eaas-space",
    "provision-cluster",
    "eaas-provision-space",
    "eaas-provision-cluster",
];

/// Read the release-test-pipeline.yaml from the cloned operator repo.
pub fn fetch_pipeline_yaml(operator_dir: &Path) -> Result<String> {
    let pipeline_path = operator_dir.join(".konflux/tekton/release-test-pipeline.yaml");
    std::fs::read_to_string(&pipeline_path).with_context(|| {
        format!(
            "Failed to read pipeline YAML at {}",
            pipeline_path.display()
        )
    })
}

/// Modify the pipeline YAML to create a standalone variant.
///
/// Removes EaaS provisioning tasks and adjusts references so the pipeline
/// runs directly on the user's current cluster. Adds an INDEX_IMAGE pipeline
/// param that deploy-operator reads instead of EaaS task results.
pub fn create_standalone_pipeline(pipeline_yaml: &str) -> Result<String> {
    let mut doc: serde_json::Value =
        serde_yaml::from_str(pipeline_yaml).context("Failed to parse pipeline YAML")?;

    // Rename the pipeline to avoid conflicts
    if let Some(name) = doc.pointer_mut("/metadata/name") {
        let orig = name.as_str().unwrap_or("release-test-pipeline");
        *name = serde_json::Value::String(format!("{}-standalone", orig));
    }

    // Add INDEX_IMAGE pipeline parameter
    if let Some(params) = doc.pointer_mut("/spec/params") {
        if let Some(arr) = params.as_array_mut() {
            let has_index_image = arr.iter().any(|p| {
                p.get("name")
                    .and_then(|n| n.as_str())
                    .map(|n| n == "INDEX_IMAGE")
                    .unwrap_or(false)
            });
            if !has_index_image {
                arr.push(serde_json::json!({
                    "name": "INDEX_IMAGE",
                    "type": "string",
                    "description": "FBC index image containing the operator bundle"
                }));
            }
        }
    }

    // Remove EaaS tasks from the pipeline
    if let Some(tasks) = doc.pointer_mut("/spec/tasks") {
        if let Some(arr) = tasks.as_array_mut() {
            arr.retain(|task| {
                let task_name = task
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                !EAAS_TASKS.iter().any(|eaas| task_name.contains(eaas))
            });

            // For remaining tasks: replace EaaS result references with pipeline params
            for task in arr.iter_mut() {
                replace_eaas_references(task);
            }
        }
    }

    // Also handle finally tasks
    if let Some(tasks) = doc.pointer_mut("/spec/finally") {
        if let Some(arr) = tasks.as_array_mut() {
            arr.retain(|task| {
                let task_name = task
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("");
                !EAAS_TASKS.iter().any(|eaas| task_name.contains(eaas))
            });

            for task in arr.iter_mut() {
                replace_eaas_references(task);
            }
        }
    }

    // Remove runAfter references to EaaS tasks
    remove_eaas_run_after(&mut doc);

    serde_yaml::to_string(&doc).context("Failed to serialize standalone pipeline YAML")
}

/// Replace references to EaaS task results with pipeline-level param references.
fn replace_eaas_references(task: &mut serde_json::Value) {
    let task_json = serde_json::to_string(task).unwrap_or_default();

    // Replace patterns like $(tasks.provision-cluster.results.clusterName) with direct values
    // and $(tasks.provision-eaas-space.results.*) references
    let mut replaced = task_json.clone();

    // Replace any EaaS task result reference for kubeconfig/cluster with empty or param ref
    for eaas_task in EAAS_TASKS {
        // Replace cluster-related results with pipeline param
        let pattern = format!("$(tasks.{}.results.", eaas_task);
        if replaced.contains(&pattern) {
            // Generic replacement: any result from EaaS tasks -> use INDEX_IMAGE param
            replaced = replaced.replace(
                &format!("$(tasks.{}.results.clusterName)", eaas_task),
                "standalone-cluster",
            );
            replaced = replaced.replace(
                &format!("$(tasks.{}.results.secretRef)", eaas_task),
                "",
            );
            // Catch-all for other result references
            while let Some(start) = replaced.find(&pattern) {
                if let Some(end) = replaced[start..].find(')') {
                    let full = &replaced[start..start + end + 1];
                    replaced = replaced.replace(full, "");
                } else {
                    break;
                }
            }
        }
    }

    if replaced != task_json {
        if let Ok(new_val) = serde_json::from_str(&replaced) {
            *task = new_val;
        }
    }
}

/// Remove runAfter references to EaaS tasks from all pipeline tasks.
fn remove_eaas_run_after(doc: &mut serde_json::Value) {
    for section in &["/spec/tasks", "/spec/finally"] {
        if let Some(tasks) = doc.pointer_mut(section) {
            if let Some(arr) = tasks.as_array_mut() {
                for task in arr.iter_mut() {
                    if let Some(run_after) = task.get_mut("runAfter") {
                        if let Some(ra_arr) = run_after.as_array_mut() {
                            ra_arr.retain(|v| {
                                let name = v.as_str().unwrap_or("");
                                !EAAS_TASKS.iter().any(|eaas| name.contains(eaas))
                            });
                        }
                    }
                }
            }
        }
    }
}

/// Orchestrate the full pipeline trigger flow.
///
/// 1. Read SNAPSHOT JSON, extract the FBC index containerImage.
/// 2. Fetch and create standalone pipeline YAML.
/// 3. Apply the Pipeline to the cluster namespace.
/// 4. Create and apply a PipelineRun with the SNAPSHOT and INDEX_IMAGE params.
/// 5. Return the PipelineRun name.
pub fn trigger_pipeline(
    snapshot_path: &Path,
    operator_dir: &Path,
    namespace: &str,
) -> Result<String> {
    // 1. Read SNAPSHOT and extract index image
    let snapshot_json = std::fs::read_to_string(snapshot_path)
        .with_context(|| format!("Failed to read snapshot at {}", snapshot_path.display()))?;
    let snapshot: serde_json::Value =
        serde_json::from_str(&snapshot_json).context("Failed to parse SNAPSHOT JSON")?;

    let index_image = extract_index_image(&snapshot)?;
    eprintln!("Index image from SNAPSHOT: {}", index_image);

    // 2. Fetch and create standalone pipeline
    let raw_yaml = fetch_pipeline_yaml(operator_dir)?;
    let standalone_yaml = create_standalone_pipeline(&raw_yaml)?;

    // 3. Apply Pipeline to cluster
    let pipeline_file = tempfile::NamedTempFile::new().context("Failed to create temp file")?;
    std::fs::write(pipeline_file.path(), &standalone_yaml)?;

    eprintln!("Applying standalone pipeline to namespace {}...", namespace);
    exec::run_cmd(
        "oc",
        &[
            "apply",
            "-f",
            pipeline_file.path().to_str().unwrap(),
            "-n",
            namespace,
        ],
    )?;

    // 4. Create PipelineRun
    let pipelinerun_name = format!(
        "streamstress-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    );

    let pipelinerun_yaml = create_pipelinerun_yaml(
        &pipelinerun_name,
        &snapshot_json,
        &index_image,
        namespace,
    );

    let pr_file = tempfile::NamedTempFile::new().context("Failed to create temp file")?;
    std::fs::write(pr_file.path(), &pipelinerun_yaml)?;

    eprintln!("Creating PipelineRun {}...", pipelinerun_name);
    exec::run_cmd(
        "oc",
        &[
            "apply",
            "-f",
            pr_file.path().to_str().unwrap(),
            "-n",
            namespace,
        ],
    )?;

    Ok(pipelinerun_name)
}

/// Extract the FBC index image from the SNAPSHOT JSON.
///
/// The SNAPSHOT has a `components` array; we look for one whose name contains
/// "fbc" or "index" and return its `containerImage`.
fn extract_index_image(snapshot: &serde_json::Value) -> Result<String> {
    let components = snapshot
        .get("components")
        .and_then(|c| c.as_array())
        .ok_or_else(|| anyhow::anyhow!("SNAPSHOT JSON missing 'components' array"))?;

    // Look for the FBC index component
    for comp in components {
        let name = comp.get("name").and_then(|n| n.as_str()).unwrap_or("");
        if name.contains("fbc") || name.contains("index") {
            if let Some(image) = comp.get("containerImage").and_then(|i| i.as_str()) {
                return Ok(image.to_string());
            }
        }
    }

    // Fallback: use the last component's image (index is typically built last)
    if let Some(last) = components.last() {
        if let Some(image) = last.get("containerImage").and_then(|i| i.as_str()) {
            return Ok(image.to_string());
        }
    }

    anyhow::bail!("No FBC index image found in SNAPSHOT components")
}

/// Generate PipelineRun YAML for the standalone pipeline.
fn create_pipelinerun_yaml(
    name: &str,
    snapshot_json: &str,
    index_image: &str,
    namespace: &str,
) -> String {
    let pr = serde_json::json!({
        "apiVersion": "tekton.dev/v1",
        "kind": "PipelineRun",
        "metadata": {
            "name": name,
            "namespace": namespace,
        },
        "spec": {
            "pipelineRef": {
                "name": "release-test-pipeline-standalone",
            },
            "params": [
                {
                    "name": "SNAPSHOT",
                    "value": snapshot_json,
                },
                {
                    "name": "INDEX_IMAGE",
                    "value": index_image,
                },
            ],
            "timeouts": {
                "pipeline": "1h30m0s",
            },
        },
    });

    serde_yaml::to_string(&pr).unwrap_or_default()
}

/// Poll a PipelineRun until completion or timeout.
///
/// Checks `oc get pipelinerun` status every 30 seconds. Default timeout is 60 minutes
/// (gauge tests typically take ~45 min).
pub fn wait_for_pipeline(
    pipelinerun_name: &str,
    namespace: &str,
    timeout_secs: u64,
) -> Result<PipelineRunResult> {
    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let poll_interval = Duration::from_secs(30);

    eprintln!(
        "Waiting for PipelineRun {} (timeout: {}m)...",
        pipelinerun_name,
        timeout_secs / 60
    );

    loop {
        if start.elapsed() > timeout {
            return Ok(PipelineRunResult {
                name: pipelinerun_name.to_string(),
                status: PipelineRunStatus::Timeout,
                duration: start.elapsed(),
                reason: "Timeout".to_string(),
            });
        }

        let result = exec::run_cmd_unchecked(
            "oc",
            &[
                "get",
                "pipelinerun",
                pipelinerun_name,
                "-n",
                namespace,
                "-o",
                "jsonpath={.status.conditions[0].reason}",
            ],
        )?;

        let reason = result.stdout.trim().to_string();

        match reason.as_str() {
            "Succeeded" => {
                eprintln!("PipelineRun {} succeeded!", pipelinerun_name);
                return Ok(PipelineRunResult {
                    name: pipelinerun_name.to_string(),
                    status: PipelineRunStatus::Succeeded,
                    duration: start.elapsed(),
                    reason,
                });
            }
            "Failed" | "PipelineRunTimeout" | "CouldntGetPipeline" => {
                eprintln!("PipelineRun {} failed: {}", pipelinerun_name, reason);
                return Ok(PipelineRunResult {
                    name: pipelinerun_name.to_string(),
                    status: PipelineRunStatus::Failed,
                    duration: start.elapsed(),
                    reason,
                });
            }
            _ => {
                // Still running (Running, Started, etc.)
                let elapsed_min = start.elapsed().as_secs() / 60;
                eprintln!(
                    "  [{}m] PipelineRun status: {}",
                    elapsed_min,
                    if reason.is_empty() { "Pending" } else { &reason }
                );
            }
        }

        std::thread::sleep(poll_interval);
    }
}

/// E2E test task names in the release-test-pipeline whose logs contain gauge output.
const E2E_TEST_TASKS: &[&str] = &[
    "e2e-test-pipelines",
    "e2e-test-metrics",
    "e2e-test-manualapprovalgate",
];

/// Step container name that runs the gauge tests inside each e2e task.
const E2E_STEP_CONTAINER: &str = "step-run-e2e-test";

/// Collect test results from PipelineRun task logs.
///
/// For each e2e test task, fetches the pod logs and parses gauge stdout
/// using the existing `results::parse_gauge_stdout` parser.
pub fn collect_results(
    pipelinerun_name: &str,
    namespace: &str,
    output_dir: &Path,
) -> Result<Vec<results::TestRunResult>> {
    let logs_dir = output_dir.join("logs");
    std::fs::create_dir_all(&logs_dir)?;

    let mut all_results = Vec::new();

    for task_name in E2E_TEST_TASKS {
        let pod_name = format!("{}-{}-pod", pipelinerun_name, task_name);

        eprintln!("Collecting results from task: {} (pod: {})", task_name, pod_name);

        // Fetch task logs from the step container
        let log_result = exec::run_cmd_unchecked(
            "oc",
            &[
                "logs",
                "-n", namespace,
                &pod_name,
                "-c", E2E_STEP_CONTAINER,
            ],
        );

        let log_output = match log_result {
            Ok(ref r) if r.exit_code == 0 && !r.stdout.is_empty() => r.stdout.clone(),
            Ok(ref r) => {
                // Try alternative pod naming: pipelinerun-taskname-random-pod
                eprintln!(
                    "  Could not get logs for {} (exit={}), trying label selector...",
                    pod_name, r.exit_code
                );
                match get_task_logs_by_label(pipelinerun_name, task_name, namespace) {
                    Ok(logs) => logs,
                    Err(e) => {
                        eprintln!("  WARNING: Skipping {}: {}", task_name, e);
                        continue;
                    }
                }
            }
            Err(e) => {
                eprintln!("  WARNING: Skipping {}: {}", task_name, e);
                continue;
            }
        };

        // Save raw log for reference
        let log_file = logs_dir.join(format!("{}.log", task_name));
        std::fs::write(&log_file, &log_output)?;

        // Parse gauge stdout using existing parser
        match results::parse_gauge_stdout_str(&log_output) {
            Ok(mut run_result) => {
                run_result.source = Some(format!("konflux-pipeline:{}", task_name));
                eprintln!(
                    "  {} -- {}/{} passed, {} failed",
                    task_name, run_result.passed, run_result.total, run_result.failed
                );
                all_results.push(run_result);
            }
            Err(e) => {
                eprintln!("  WARNING: Failed to parse results from {}: {}", task_name, e);
            }
        }
    }

    Ok(all_results)
}

/// Get task logs using label selector when pod name doesn't match convention.
fn get_task_logs_by_label(
    pipelinerun_name: &str,
    task_name: &str,
    namespace: &str,
) -> Result<String> {
    let label = format!(
        "tekton.dev/pipelineRun={},tekton.dev/pipelineTask={}",
        pipelinerun_name, task_name
    );

    // Get pod name via label
    let pod_result = exec::run_cmd(
        "oc",
        &[
            "get", "pods",
            "-n", namespace,
            "-l", &label,
            "-o", "jsonpath={.items[0].metadata.name}",
        ],
    )?;

    let actual_pod = pod_result.stdout.trim().to_string();
    if actual_pod.is_empty() {
        anyhow::bail!("No pod found for task {}", task_name);
    }

    let log_result = exec::run_cmd(
        "oc",
        &["logs", "-n", namespace, &actual_pod, "-c", E2E_STEP_CONTAINER],
    )?;

    Ok(log_result.stdout)
}

/// Save collected pipeline results to output_dir/results/results.json.
///
/// Merges results from all e2e test tasks into a single CategorizedTestRunResult,
/// and copies the SNAPSHOT JSON to output_dir for QE reference.
pub fn save_konflux_results(
    task_results: &[results::TestRunResult],
    snapshot_path: &Path,
    output_dir: &Path,
) -> Result<()> {
    let results_dir = output_dir.join("results");
    std::fs::create_dir_all(&results_dir)?;

    // Merge all task results into one combined TestRunResult
    let mut all_tests = Vec::new();
    let mut total_duration = 0.0;

    for tr in task_results {
        all_tests.extend(tr.tests.clone());
        total_duration += tr.duration_secs;
    }

    let passed = all_tests.iter().filter(|t| t.passed).count();
    let failed = all_tests.iter().filter(|t| !t.passed).count();

    let combined = results::TestRunResult {
        total: all_tests.len(),
        passed,
        failed,
        errors: 0,
        duration_secs: total_duration,
        source: Some("konflux-pipeline".to_string()),
        tests: all_tests,
    };

    // Categorize and write
    let categorized = results::categorize_results(&combined);
    let json_path = results_dir.join("results.json");
    results::write_categorized_json(&categorized, &json_path)?;

    // Copy SNAPSHOT JSON to output_dir for QE reference
    if snapshot_path.exists() {
        let dest = output_dir.join("snapshot.json");
        std::fs::copy(snapshot_path, &dest)
            .with_context(|| format!("Failed to copy snapshot to {}", dest.display()))?;
        eprintln!("SNAPSHOT copied to {}", dest.display());
    }

    Ok(())
}

/// Print a summary table of results per test suite to stdout.
pub fn print_pipeline_summary(task_results: &[results::TestRunResult]) {
    use console::Style;

    let green = Style::new().green().bold();
    let red = Style::new().red().bold();
    let bold = Style::new().bold();

    println!();
    println!("{}", bold.apply_to("Pipeline Test Results Summary"));
    println!("{}", "=".repeat(60));
    println!(
        "{:<30} {:>8} {:>8} {:>8}",
        "Suite", "Total", "Passed", "Failed"
    );
    println!("{}", "-".repeat(60));

    let mut grand_total = 0;
    let mut grand_passed = 0;
    let mut grand_failed = 0;

    for tr in task_results {
        let suite_name = tr
            .source
            .as_deref()
            .unwrap_or("unknown")
            .strip_prefix("konflux-pipeline:")
            .unwrap_or("unknown");

        let passed_str = format!("{}", tr.passed);
        let failed_str = format!("{}", tr.failed);

        println!(
            "{:<30} {:>8} {:>8} {:>8}",
            suite_name,
            tr.total,
            green.apply_to(&passed_str),
            if tr.failed > 0 {
                red.apply_to(&failed_str).to_string()
            } else {
                failed_str
            }
        );

        grand_total += tr.total;
        grand_passed += tr.passed;
        grand_failed += tr.failed;
    }

    println!("{}", "-".repeat(60));
    println!(
        "{:<30} {:>8} {:>8} {:>8}",
        bold.apply_to("TOTAL"),
        grand_total,
        green.apply_to(format!("{}", grand_passed)),
        if grand_failed > 0 {
            red.apply_to(format!("{}", grand_failed)).to_string()
        } else {
            format!("{}", grand_failed)
        }
    );
    println!();
}
