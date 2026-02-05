use anyhow::Result;
use console::Style;

use crate::exec::run_cmd_unchecked;
use crate::progress::{finish_spinner, stage_spinner};
use crate::types::CheckResult;

struct ToolSpec {
    name: &'static str,
    version_args: &'static [&'static str],
    fix_hint: &'static str,
}

const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "oc",
        version_args: &["version", "--client"],
        fix_hint: "Install the OpenShift CLI: https://docs.openshift.com/container-platform/latest/cli_reference/openshift_cli/getting-started-cli.html",
    },
    ToolSpec {
        name: "ko",
        version_args: &["version"],
        fix_hint: "Install ko: go install github.com/google/ko@latest",
    },
    ToolSpec {
        name: "git",
        version_args: &["version"],
        fix_hint: "Install git from https://git-scm.com",
    },
    ToolSpec {
        name: "go",
        version_args: &["version"],
        fix_hint: "Install Go from https://go.dev/dl/",
    },
    ToolSpec {
        name: "gh",
        version_args: &["version"],
        fix_hint: "Install GitHub CLI: brew install gh && gh auth login",
    },
];

pub fn run_check(_verbose: bool) -> Result<bool> {
    let mut results: Vec<CheckResult> = Vec::new();

    for tool in TOOLS {
        let pb = stage_spinner(&format!("Checking {}...", tool.name));

        let result = if which::which(tool.name).is_ok() {
            match run_cmd_unchecked(tool.name, tool.version_args) {
                Ok(exec) => {
                    let detail = exec.stdout.lines().next().unwrap_or("").trim().to_string();
                    CheckResult {
                        name: tool.name.to_string(),
                        passed: true,
                        detail,
                        fix_hint: None,
                    }
                }
                Err(_) => CheckResult {
                    name: tool.name.to_string(),
                    passed: false,
                    detail: "Found on PATH but failed to get version".to_string(),
                    fix_hint: Some(tool.fix_hint.to_string()),
                },
            }
        } else {
            CheckResult {
                name: tool.name.to_string(),
                passed: false,
                detail: "Not found on PATH".to_string(),
                fix_hint: Some(tool.fix_hint.to_string()),
            }
        };

        finish_spinner(&pb, result.passed);
        results.push(result);
    }

    // Cluster auth check
    let cluster_connected;
    {
        let pb = stage_spinner("Checking cluster auth...");
        let result = match run_cmd_unchecked("oc", &["whoami"]) {
            Ok(exec) if exec.exit_code == 0 => {
                cluster_connected = true;
                CheckResult {
                    name: "cluster auth".to_string(),
                    passed: true,
                    detail: exec.stdout.trim().to_string(),
                    fix_hint: None,
                }
            }
            _ => {
                cluster_connected = false;
                CheckResult {
                    name: "cluster auth".to_string(),
                    passed: false,
                    detail: "Not logged in to any cluster".to_string(),
                    fix_hint: Some("Log in to your OpenShift cluster: oc login <cluster-url>".to_string()),
                }
            }
        };
        finish_spinner(&pb, result.passed);
        results.push(result);
    }

    // Operator check (only if connected to cluster)
    if cluster_connected {
        let pb = stage_spinner("Checking OpenShift Pipelines operator...");
        let result = match run_cmd_unchecked("oc", &["get", "tektonconfigs.operator.tekton.dev", "config"]) {
            Ok(exec) if exec.exit_code == 0 => CheckResult {
                name: "pipelines operator".to_string(),
                passed: true,
                detail: "TektonConfig CR found".to_string(),
                fix_hint: None,
            },
            _ => CheckResult {
                name: "pipelines operator".to_string(),
                passed: false,
                detail: "OpenShift Pipelines operator not installed".to_string(),
                fix_hint: Some("Will be auto-installed by build/deploy/run commands".to_string()),
            },
        };
        finish_spinner(&pb, result.passed);
        results.push(result);
    }

    // Registry route check
    {
        let pb = stage_spinner("Checking registry route...");
        let result = if !cluster_connected {
            CheckResult {
                name: "registry route".to_string(),
                passed: false,
                detail: "SKIP - not connected to cluster".to_string(),
                fix_hint: None,
            }
        } else {
            match run_cmd_unchecked(
                "oc",
                &[
                    "get", "route", "default-route",
                    "-n", "openshift-image-registry",
                    "-o", "jsonpath={.spec.host}",
                ],
            ) {
                Ok(exec) if exec.exit_code == 0 && !exec.stdout.trim().is_empty() => {
                    CheckResult {
                        name: "registry route".to_string(),
                        passed: true,
                        detail: exec.stdout.trim().to_string(),
                        fix_hint: None,
                    }
                }
                _ => CheckResult {
                    name: "registry route".to_string(),
                    passed: false,
                    detail: "Default registry route not found".to_string(),
                    fix_hint: Some(
                        "Enable the default registry route:\n  oc patch configs.imageregistry.operator.openshift.io/cluster --patch '{\"spec\":{\"defaultRoute\":true}}' --type=merge"
                            .to_string(),
                    ),
                },
            }
        };
        finish_spinner(&pb, result.passed);
        results.push(result);
    }

    // Print summary
    println!();
    let green = Style::new().green().bold();
    let red = Style::new().red().bold();

    for r in &results {
        let auto_fixable = matches!(r.name.as_str(), "registry route" | "pipelines operator");
        if r.passed {
            println!("  {} {}: {}", green.apply_to("PASS"), r.name, r.detail);
        } else {
            let suffix = if auto_fixable { " [auto-fixable]" } else { "" };
            println!("  {} {}: {}{}", red.apply_to("FAIL"), r.name, r.detail, suffix);
            if let Some(hint) = &r.fix_hint {
                println!("       hint: {hint}");
            }
        }
    }
    println!();

    Ok(results.iter().all(|r| r.passed))
}
