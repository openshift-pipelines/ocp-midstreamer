use std::collections::HashMap;
use std::process::Command;

use serde::Serialize;

use crate::component::ComponentSpec;
use crate::config::ComponentConfig;
use crate::github;

/// Resolved component info for dry-run display.
#[derive(Debug, Serialize)]
pub struct ResolvedComponent {
    pub name: String,
    pub repo_url: String,
    pub git_ref: String,
    pub resolved_sha: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub as_of_date: Option<String>,
    pub import_paths: Vec<String>,
    pub image_names: Vec<String>,
}

/// Resolve all component specs to ResolvedComponent with SHA lookup.
pub fn resolve_components(
    specs: &[ComponentSpec],
    configs: &HashMap<String, ComponentConfig>,
) -> Vec<ResolvedComponent> {
    specs
        .iter()
        .filter_map(|spec| {
            let cfg = configs.get(&spec.name)?;
            let git_ref_display = spec.git_ref.as_deref().unwrap_or("HEAD").to_string();
            let resolved_sha = resolve_sha(&cfg.repo, spec.git_ref.as_deref());
            let image_names: Vec<String> = cfg
                .import_paths
                .iter()
                .filter_map(|p| p.rsplit('/').next())
                .map(|s| s.to_string())
                .collect();
            Some(ResolvedComponent {
                name: spec.name.clone(),
                repo_url: cfg.repo.clone(),
                git_ref: git_ref_display,
                resolved_sha,
                commit_date: None,
                commit_message: None,
                as_of_date: None,
                import_paths: cfg.import_paths.clone(),
                image_names,
            })
        })
        .collect()
}

/// Resolve all component specs to ResolvedComponent with SHA lookup and optional as-of date resolution.
///
/// When `as_of` is provided, components without explicit git refs will resolve to the commit
/// that was HEAD at end-of-day UTC on that date. Components with explicit refs ignore as_of.
pub fn resolve_components_with_date(
    specs: &[ComponentSpec],
    configs: &HashMap<String, ComponentConfig>,
    as_of: Option<&str>,
) -> Vec<ResolvedComponent> {
    specs
        .iter()
        .filter_map(|spec| {
            let cfg = configs.get(&spec.name)?;

            // Determine effective ref: explicit git_ref > as_of_date > HEAD
            let (git_ref_display, resolved_sha, commit_date, commit_message, as_of_used) =
                if let Some(ref r) = spec.git_ref {
                    // Explicit ref takes priority
                    let sha = resolve_sha(&cfg.repo, Some(r.as_str()));
                    (r.clone(), sha, None, None, None)
                } else if let Some(date) = as_of.or(spec.as_of_date.as_deref()) {
                    // Resolve from as-of date
                    match github::resolve_commit_before_date(&cfg.repo, date) {
                        Ok(info) => {
                            let sha = info.sha[..std::cmp::min(info.sha.len(), 12)].to_string();
                            (
                                format!("as-of:{}", date),
                                sha,
                                Some(info.date),
                                Some(info.message),
                                Some(date.to_string()),
                            )
                        }
                        Err(e) => {
                            eprintln!(
                                "WARNING: Could not resolve as-of date for {}: {}",
                                spec.name, e
                            );
                            ("HEAD".to_string(), resolve_sha(&cfg.repo, None), None, None, None)
                        }
                    }
                } else {
                    // Default: HEAD
                    ("HEAD".to_string(), resolve_sha(&cfg.repo, None), None, None, None)
                };

            let image_names: Vec<String> = cfg
                .import_paths
                .iter()
                .filter_map(|p| p.rsplit('/').next())
                .map(|s| s.to_string())
                .collect();

            Some(ResolvedComponent {
                name: spec.name.clone(),
                repo_url: cfg.repo.clone(),
                git_ref: git_ref_display,
                resolved_sha,
                commit_date,
                commit_message,
                as_of_date: as_of_used,
                import_paths: cfg.import_paths.clone(),
                image_names,
            })
        })
        .collect()
}

/// Resolve a git ref to a commit SHA using `git ls-remote`.
/// Returns "N/A" on failure.
pub fn resolve_sha(repo_url: &str, git_ref: Option<&str>) -> String {
    let ref_arg = match git_ref {
        Some(r) => crate::component::resolve_git_ref(r),
        None => "HEAD".to_string(),
    };

    let output = Command::new("git")
        .args(["ls-remote", repo_url, &ref_arg])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            // First column is the SHA
            stdout
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().next())
                .map(|sha| sha[..std::cmp::min(sha.len(), 12)].to_string())
                .unwrap_or_else(|| "N/A".to_string())
        }
        _ => "N/A".to_string(),
    }
}

/// Print a human-readable table of resolved components.
pub fn print_table(resolved: &[ResolvedComponent]) {
    // Check if any component has as-of resolution (to decide column format)
    let has_as_of = resolved.iter().any(|r| r.as_of_date.is_some());

    if has_as_of {
        println!(
            "{:<12} {:<50} {:<14} {:<14} {:<12} {}",
            "COMPONENT", "REPO", "REF", "COMMIT SHA", "DATE", "MESSAGE"
        );
        println!(
            "{:<12} {:<50} {:<14} {:<14} {:<12} {}",
            "---------", "----", "---", "----------", "----", "-------"
        );
        for rc in resolved {
            let date_display = rc
                .commit_date
                .as_ref()
                .map(|d| &d[..10.min(d.len())]) // Just YYYY-MM-DD part
                .unwrap_or("-");
            let msg_display = rc.commit_message.as_deref().unwrap_or("-");
            // Truncate message to 40 chars
            let msg_truncated = if msg_display.len() > 40 {
                format!("{}...", &msg_display[..37])
            } else {
                msg_display.to_string()
            };
            println!(
                "{:<12} {:<50} {:<14} {:<14} {:<12} {}",
                rc.name, rc.repo_url, rc.git_ref, rc.resolved_sha, date_display, msg_truncated
            );
        }
    } else {
        // Original format without date/message columns
        println!(
            "{:<12} {:<50} {:<12} {:<14} {}",
            "COMPONENT", "REPO", "REF", "COMMIT SHA", "IMAGES"
        );
        println!(
            "{:<12} {:<50} {:<12} {:<14} {}",
            "---------", "----", "---", "----------", "------"
        );
        for rc in resolved {
            println!(
                "{:<12} {:<50} {:<12} {:<14} {}",
                rc.name,
                rc.repo_url,
                rc.git_ref,
                rc.resolved_sha,
                rc.image_names.join(", ")
            );
        }
    }
}

/// Print resolved components as JSON.
pub fn print_json(resolved: &[ResolvedComponent]) {
    match serde_json::to_string_pretty(resolved) {
        Ok(json) => println!("{json}"),
        Err(e) => eprintln!("Error serializing JSON: {e}"),
    }
}
