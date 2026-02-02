use anyhow::{Context, Result};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct ExecResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub duration: Duration,
}

/// Run a command and return an error if it exits non-zero.
pub fn run_cmd(cmd: &str, args: &[&str]) -> Result<ExecResult> {
    let result = run_cmd_unchecked(cmd, args)?;
    if result.exit_code != 0 {
        anyhow::bail!(
            "{} {} failed (exit {}): {}",
            cmd,
            args.join(" "),
            result.exit_code,
            result.stderr.trim()
        );
    }
    Ok(result)
}

/// Run a command with environment variables and return an error if it exits non-zero.
pub fn run_cmd_with_env(cmd: &str, args: &[&str], envs: &[(&str, &str)]) -> Result<ExecResult> {
    let start = Instant::now();
    let output = Command::new(cmd)
        .args(args)
        .envs(envs.iter().cloned())
        .output()
        .with_context(|| format!("failed to execute {cmd}"))?;
    let duration = start.elapsed();

    let result = ExecResult {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        duration,
    };

    if result.exit_code != 0 {
        anyhow::bail!(
            "{} {} failed (exit {}): {}",
            cmd,
            args.join(" "),
            result.exit_code,
            result.stderr.trim()
        );
    }
    Ok(result)
}

/// Run a command with streaming output (stdout/stderr inherited by terminal).
/// Returns Ok(exit_code) on success (exit 0), or an error on non-zero exit.
pub fn run_cmd_streaming(cmd: &str, args: &[&str], envs: &[(&str, &str)]) -> Result<i32> {
    let status = Command::new(cmd)
        .args(args)
        .envs(envs.iter().cloned())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("failed to execute {cmd}"))?;

    let code = status.code().unwrap_or(-1);
    if code != 0 {
        anyhow::bail!("{} failed with exit code {}", cmd, code);
    }
    Ok(code)
}

/// Run a command and return the result regardless of exit code.
pub fn run_cmd_unchecked(cmd: &str, args: &[&str]) -> Result<ExecResult> {
    let start = Instant::now();
    let output = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("failed to execute {cmd}"))?;
    let duration = start.elapsed();

    Ok(ExecResult {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        duration,
    })
}
