//! GitHub API module for date-to-SHA resolution using the gh CLI.
//!
//! This module provides functionality to resolve a date to the commit SHA that was
//! HEAD at end-of-day UTC for that date. This is the foundation for historical builds.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;

/// Information about a commit returned from the GitHub API.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CommitInfo {
    pub sha: String,
    pub date: String,
    pub message: String,
}

/// Parse owner and repo from a GitHub URL.
///
/// Handles both `https://github.com/owner/repo.git` and `https://github.com/owner/repo`
///
/// # Examples
/// ```ignore
/// let (owner, repo) = parse_github_url("https://github.com/tektoncd/pipeline")?;
/// assert_eq!(owner, "tektoncd");
/// assert_eq!(repo, "pipeline");
/// ```
pub fn parse_github_url(url: &str) -> Result<(String, String)> {
    let url = url.trim_end_matches(".git");
    let parts: Vec<&str> = url.rsplitn(3, '/').collect();
    if parts.len() >= 2 {
        Ok((parts[1].to_string(), parts[0].to_string()))
    } else {
        anyhow::bail!(
            "Invalid GitHub URL: {}. Expected format: https://github.com/owner/repo",
            url
        )
    }
}

/// Resolve the latest commit before a given date using GitHub API via gh CLI.
///
/// This function queries the GitHub commits API for commits up to end-of-day UTC
/// on the given date, returning the most recent commit.
///
/// # Arguments
/// * `repo_url` - GitHub repository URL (e.g., `https://github.com/tektoncd/pipeline`)
/// * `date` - Date in YYYY-MM-DD format
///
/// # Returns
/// `CommitInfo` with sha, date, and first line of commit message
///
/// # Errors
/// - Returns error if the URL is not a valid GitHub URL
/// - Returns error if gh CLI is not installed
/// - Returns error if rate limit is exceeded (suggests `gh auth login`)
/// - Returns error if repository is not found
/// - Returns error if no commits exist before the given date
///
/// # Example
/// ```ignore
/// let commit = resolve_commit_before_date(
///     "https://github.com/tektoncd/pipeline",
///     "2024-01-15"
/// )?;
/// println!("Commit {} from {}: {}", commit.sha, commit.date, commit.message);
/// ```
pub fn resolve_commit_before_date(repo_url: &str, date: &str) -> Result<CommitInfo> {
    let (owner, repo) = parse_github_url(repo_url)?;

    // Append end-of-day UTC for consistent behavior
    let until = format!("{}T23:59:59Z", date);

    let output = Command::new("gh")
        .args([
            "api",
            &format!(
                "repos/{}/{}/commits?per_page=1&until={}",
                owner, repo, until
            ),
            "--jq",
            ".[0] | {sha: .sha, date: .commit.author.date, message: (.commit.message | split(\"\\n\")[0])}",
        ])
        .output()
        .context("Failed to execute gh api - is gh CLI installed? Run: brew install gh")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("rate limit") {
            anyhow::bail!(
                "GitHub API rate limit exceeded. Run `gh auth login` to authenticate for higher limits."
            );
        }
        if stderr.contains("Could not resolve") || stderr.contains("Not Found") {
            anyhow::bail!(
                "Repository not found: {}/{}. Check the URL is correct.",
                owner,
                repo
            );
        }
        anyhow::bail!("gh api failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() || stdout.trim() == "null" {
        anyhow::bail!(
            "No commits found before {} in {}/{}. The repository may not have existed yet on that date.",
            date,
            owner,
            repo
        );
    }

    let info: CommitInfo = serde_json::from_str(&stdout).with_context(|| {
        format!(
            "Failed to parse commit info from GitHub API response: {}",
            stdout.trim()
        )
    })?;

    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_github_url_standard() {
        let (owner, repo) = parse_github_url("https://github.com/tektoncd/pipeline").unwrap();
        assert_eq!(owner, "tektoncd");
        assert_eq!(repo, "pipeline");
    }

    #[test]
    fn test_parse_github_url_with_git_suffix() {
        let (owner, repo) =
            parse_github_url("https://github.com/tektoncd/pipeline.git").unwrap();
        assert_eq!(owner, "tektoncd");
        assert_eq!(repo, "pipeline");
    }

    #[test]
    fn test_parse_github_url_invalid() {
        let result = parse_github_url("not-a-url");
        assert!(result.is_err());
    }
}
