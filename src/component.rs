use anyhow::Result;
use regex::Regex;
use std::path::Path;
use std::process::Command;

use crate::exec;

/// Known component names that can be selected via --components.
pub const KNOWN_COMPONENTS: &[&str] = &[
    "pipeline", "triggers", "chains", "results",
    "manual-approval-gate", "console-plugin",
];

/// A component to build/deploy/test, with an optional git ref or as-of date override.
#[derive(Debug, Clone)]
pub struct ComponentSpec {
    pub name: String,
    pub git_ref: Option<String>,
    /// Date in YYYY-MM-DD format for historical builds. Populated from --as-of flag.
    pub as_of_date: Option<String>,
}

/// Parse a comma-separated component spec string.
///
/// Format: `name[:ref],name[:ref],...`
/// Examples:
///   - `pipeline,triggers` (default refs)
///   - `pipeline:pr/123,triggers:v0.28.0` (custom refs)
pub fn parse_component_specs(input: &str) -> Result<Vec<ComponentSpec>, String> {
    let mut specs = Vec::new();
    for part in input.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (name, git_ref) = match part.split_once(':') {
            Some((n, r)) => (n.trim(), Some(r.trim().to_string())),
            None => (part, None),
        };
        if !KNOWN_COMPONENTS.contains(&name) {
            return Err(format!(
                "Unknown component '{}'. Known: {}",
                name,
                KNOWN_COMPONENTS.join(", ")
            ));
        }
        specs.push(ComponentSpec {
            name: name.to_string(),
            git_ref,
            as_of_date: None, // Populated later from --as-of flag
        });
    }
    if specs.is_empty() {
        return Err("No components specified".to_string());
    }
    Ok(specs)
}

/// Return specs for all known components with default refs.
pub fn default_specs() -> Vec<ComponentSpec> {
    KNOWN_COMPONENTS
        .iter()
        .map(|name| ComponentSpec {
            name: name.to_string(),
            git_ref: None,
            as_of_date: None,
        })
        .collect()
}

/// Apply an as-of date to all component specs that don't have an explicit git_ref.
///
/// Components with explicit refs (e.g., "pipeline:v0.50.0") are not modified since
/// the user specified an explicit version to use.
pub fn apply_as_of_date(specs: &mut [ComponentSpec], as_of: &str) {
    for spec in specs {
        if spec.git_ref.is_none() {
            spec.as_of_date = Some(as_of.to_string());
        }
    }
}

/// Validate date format is YYYY-MM-DD.
///
/// Used by clap's value_parser for the --as-of flag.
pub fn validate_date_format(s: &str) -> std::result::Result<String, String> {
    let re = Regex::new(r"^\d{4}-(0[1-9]|1[0-2])-(0[1-9]|[12]\d|3[01])$").expect("Invalid regex");
    if re.is_match(s) {
        Ok(s.to_string())
    } else {
        Err("Date must be in YYYY-MM-DD format (e.g., 2024-01-15)".to_string())
    }
}

/// Resolve a user-provided git ref to a fetchable refspec.
///
/// Maps `pr/NNN` to `refs/pull/NNN/head`; passes through everything else.
pub fn resolve_git_ref(user_ref: &str) -> String {
    if let Some(pr_num) = user_ref.strip_prefix("pr/") {
        format!("refs/pull/{}/head", pr_num)
    } else {
        user_ref.to_string()
    }
}

/// Clone a repo with an optional git ref.
///
/// If `git_ref` is Some: git init, fetch the resolved ref, checkout FETCH_HEAD.
/// If `git_ref` is None: shallow clone default branch.
pub fn clone_with_ref(repo_url: &str, dest: &Path, git_ref: Option<&str>) -> Result<()> {
    match git_ref {
        Some(r) => {
            let resolved = resolve_git_ref(r);
            let dest_str = dest.to_str().unwrap_or_default();

            // git init
            exec::run_cmd("git", &["init", dest_str])?;

            // git fetch --depth 1 <repo> <resolved_ref>
            let status = Command::new("git")
                .args(["fetch", "--depth", "1", repo_url, &resolved])
                .current_dir(dest)
                .status()
                .map_err(|e| anyhow::anyhow!("failed to execute git fetch: {e}"))?;
            if !status.success() {
                anyhow::bail!("git fetch failed for ref '{}' (resolved: '{}')", r, resolved);
            }

            // git checkout FETCH_HEAD
            let status = Command::new("git")
                .args(["checkout", "FETCH_HEAD"])
                .current_dir(dest)
                .status()
                .map_err(|e| anyhow::anyhow!("failed to execute git checkout: {e}"))?;
            if !status.success() {
                anyhow::bail!("git checkout FETCH_HEAD failed");
            }

            Ok(())
        }
        None => {
            let dest_str = dest.to_str().unwrap_or_default();
            exec::run_cmd("git", &["clone", "--depth", "1", repo_url, dest_str])?;
            Ok(())
        }
    }
}
