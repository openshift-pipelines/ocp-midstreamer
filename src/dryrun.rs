use std::collections::HashMap;
use std::process::Command;

use serde::Serialize;

use crate::component::ComponentSpec;
use crate::config::ComponentConfig;

/// Resolved component info for dry-run display.
#[derive(Debug, Serialize)]
pub struct ResolvedComponent {
    pub name: String,
    pub repo_url: String,
    pub git_ref: String,
    pub resolved_sha: String,
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

/// Print resolved components as JSON.
pub fn print_json(resolved: &[ResolvedComponent]) {
    match serde_json::to_string_pretty(resolved) {
        Ok(json) => println!("{json}"),
        Err(e) => eprintln!("Error serializing JSON: {e}"),
    }
}
