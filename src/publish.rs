use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::process::Command;

/// Publish test results to the gh-pages branch for the dashboard.
pub fn publish(output_dir: &str, remote: Option<&str>, label: Option<&str>) -> Result<()> {
    // 1. Read results JSON
    let results_path = Path::new(output_dir).join("results/results.json");
    if !results_path.exists() {
        anyhow::bail!(
            "Results not found at {}. Run `streamstress results` first.",
            results_path.display()
        );
    }
    let results_str = fs::read_to_string(&results_path)
        .with_context(|| format!("Failed to read {}", results_path.display()))?;
    let mut run_data: serde_json::Value =
        serde_json::from_str(&results_str).context("Failed to parse results JSON")?;

    // 1b. Check for metadata.json (as-of date tracking)
    let metadata_path = Path::new(output_dir).join("results/metadata.json");
    if metadata_path.exists() {
        if let Ok(meta_str) = fs::read_to_string(&metadata_path) {
            if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&meta_str) {
                // Merge as_of_date into run data
                if let Some(as_of) = meta.get("as_of_date") {
                    run_data["as_of_date"] = as_of.clone();
                    eprintln!("Including as_of_date: {} in run data", as_of);
                }
                // Merge resolved_components into run data
                if let Some(components) = meta.get("resolved_components") {
                    run_data["component_refs"] = components.clone();
                }
            }
        }
    }

    // 1c. Check for performance results (perf/perf-results.json)
    let perf_results_path = Path::new(output_dir).join("perf/perf-results.json");
    if perf_results_path.exists() {
        if let Ok(perf_str) = fs::read_to_string(&perf_results_path) {
            if let Ok(perf_data) = serde_json::from_str::<serde_json::Value>(&perf_str) {
                run_data["performance"] = perf_data;
                eprintln!("Including performance test results in run data");
            }
        }
    }

    // 1d. Check for performance resource profile (perf/resource-profile.json)
    let perf_resource_path = Path::new(output_dir).join("perf/resource-profile.json");
    if perf_resource_path.exists() {
        if let Ok(resource_str) = fs::read_to_string(&perf_resource_path) {
            if let Ok(resource_data) = serde_json::from_str::<serde_json::Value>(&resource_str) {
                run_data["performance_resources"] = resource_data;
                eprintln!("Including performance resource profile in run data");
            }
        }
    }

    // 2. Generate run metadata
    let timestamp = chrono_utc_now();
    let run_id = format!("run-{}", timestamp.replace([':', '-', 'T'], "").replace('Z', ""));

    run_data["id"] = serde_json::json!(run_id);
    run_data["timestamp"] = serde_json::json!(timestamp);
    if let Some(lbl) = label {
        run_data["label"] = serde_json::json!(lbl);
    }

    // Truncate error_messages to 500 chars
    truncate_error_messages(&mut run_data, 500);

    // 3. Determine remote
    let remote_url = match remote {
        Some(r) => r.to_string(),
        None => detect_remote()?,
    };
    eprintln!("Publishing to: {}", remote_url);

    // 4. Clone gh-pages into tempdir
    let tmp = tempfile::tempdir().context("Failed to create temp dir")?;
    let work = tmp.path();

    let branch_exists = gh_pages_exists(&remote_url);

    if branch_exists {
        // Clone existing gh-pages
        let status = Command::new("git")
            .args(["clone", "--branch", "gh-pages", "--single-branch", "--depth", "1", &remote_url, "."])
            .current_dir(work)
            .status()
            .context("Failed to clone gh-pages")?;
        if !status.success() {
            anyhow::bail!("Failed to clone gh-pages branch");
        }
    } else {
        // Bootstrap: init orphan branch
        eprintln!("gh-pages branch not found, bootstrapping...");
        run_git(work, &["init"])?;
        run_git(work, &["checkout", "--orphan", "gh-pages"])?;
        run_git(work, &["remote", "add", "origin", &remote_url])?;

        // Create runs directory with empty manifest
        let runs_dir = work.join("runs");
        fs::create_dir_all(&runs_dir)?;
        let empty_manifest = serde_json::json!({"runs": []});
        fs::write(
            runs_dir.join("manifest.json"),
            serde_json::to_string_pretty(&empty_manifest)?,
        )?;
    }

    // 5. Copy dashboard assets (every publish, so updates propagate)
    copy_dashboard_assets(work)?;

    // 6. Write run file
    let runs_dir = work.join("runs");
    fs::create_dir_all(&runs_dir)?;
    let run_file = runs_dir.join(format!("{}.json", run_id));
    fs::write(
        &run_file,
        serde_json::to_string_pretty(&run_data)?,
    )?;
    eprintln!("Wrote run file: {}", run_id);

    // 7. Update manifest: prepend new entry
    let manifest_path = runs_dir.join("manifest.json");
    let mut manifest: serde_json::Value = if manifest_path.exists() {
        let s = fs::read_to_string(&manifest_path)?;
        serde_json::from_str(&s).unwrap_or_else(|_| serde_json::json!({"runs": []}))
    } else {
        serde_json::json!({"runs": []})
    };

    let entry = serde_json::json!({
        "id": run_id,
        "timestamp": timestamp,
        "label": label.unwrap_or(""),
        "total": run_data.get("total").and_then(|v| v.as_u64()).unwrap_or(0),
        "passed": run_data.get("passed").and_then(|v| v.as_u64()).unwrap_or(0),
        "failed": run_data.get("failed").and_then(|v| v.as_u64()).unwrap_or(0),
        "file": format!("runs/{}.json", run_id),
    });

    if let Some(runs) = manifest.get_mut("runs").and_then(|v| v.as_array_mut()) {
        runs.insert(0, entry);
    }

    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest)?,
    )?;

    // 8. Commit and push
    run_git(work, &["add", "-A"])?;
    let commit_msg = format!("publish: {} ({} total, {} passed)", run_id,
        run_data.get("total").and_then(|v| v.as_u64()).unwrap_or(0),
        run_data.get("passed").and_then(|v| v.as_u64()).unwrap_or(0),
    );
    run_git(work, &["commit", "-m", &commit_msg])?;

    // Push with one retry on failure (pull --rebase)
    if push_with_retry(work).is_err() {
        anyhow::bail!("Failed to push to gh-pages after retry");
    }

    eprintln!("Published {} to gh-pages", run_id);
    Ok(())
}

fn detect_remote() -> Result<String> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .output()
        .context("Failed to run git remote get-url")?;
    if !output.status.success() {
        anyhow::bail!("No git remote 'origin' found. Use --remote to specify.");
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn gh_pages_exists(remote_url: &str) -> bool {
    Command::new("git")
        .args(["ls-remote", "--heads", remote_url, "gh-pages"])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

fn run_git(dir: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .with_context(|| format!("Failed to run git {}", args.join(" ")))?;
    if !status.success() {
        anyhow::bail!("git {} failed", args.join(" "));
    }
    Ok(())
}

fn push_with_retry(dir: &Path) -> Result<()> {
    let first = Command::new("git")
        .args(["push", "origin", "gh-pages"])
        .current_dir(dir)
        .status();
    match first {
        Ok(s) if s.success() => return Ok(()),
        _ => {
            eprintln!("Push failed, retrying with pull --rebase...");
            run_git(dir, &["pull", "--rebase", "origin", "gh-pages"])?;
            run_git(dir, &["push", "origin", "gh-pages"])?;
        }
    }
    Ok(())
}

fn copy_dashboard_assets(dest: &Path) -> Result<()> {
    // Find dashboard/ relative to the binary or from known location
    // In practice, we look for it in the repo root via git
    let repo_root = find_repo_root()?;
    let dashboard_src = repo_root.join("dashboard");

    if !dashboard_src.exists() {
        anyhow::bail!(
            "Dashboard assets not found at {}. Ensure dashboard/ exists in repo root.",
            dashboard_src.display()
        );
    }

    copy_dir_recursive(&dashboard_src, dest)?;
    Ok(())
}

fn find_repo_root() -> Result<std::path::PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("Failed to find git repo root")?;
    if !output.status.success() {
        anyhow::bail!("Not in a git repository");
    }
    Ok(std::path::PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

fn copy_dir_recursive(src: &Path, dest: &Path) -> Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());

        if src_path.is_dir() {
            fs::create_dir_all(&dest_path)?;
            copy_dir_recursive(&src_path, &dest_path)?;
        } else {
            fs::copy(&src_path, &dest_path)?;
        }
    }
    Ok(())
}

fn truncate_error_messages(value: &mut serde_json::Value, max_len: usize) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(msg) = map.get_mut("error_message") {
                if let Some(s) = msg.as_str() {
                    if s.len() > max_len {
                        *msg = serde_json::Value::String(format!("{}...", &s[..max_len]));
                    }
                }
            }
            for v in map.values_mut() {
                truncate_error_messages(v, max_len);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                truncate_error_messages(v, max_len);
            }
        }
        _ => {}
    }
}

fn chrono_utc_now() -> String {
    // Use system command to get UTC time in ISO 8601 format
    // Avoids adding chrono dependency
    Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}
