use anyhow::{Context, Result};
use console::Style;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

// --- JUnit XML deserialization structs ---

#[derive(Debug, Deserialize)]
#[serde(rename = "testsuites")]
struct JUnitTestSuites {
    #[serde(rename = "testsuite", default)]
    suites: Vec<JUnitTestSuite>,
}

#[derive(Debug, Deserialize)]
struct JUnitTestSuite {
    #[serde(rename = "@tests")]
    #[allow(dead_code)]
    tests: Option<usize>,
    #[serde(rename = "@failures")]
    #[allow(dead_code)]
    failures: Option<usize>,
    #[serde(rename = "@errors")]
    #[allow(dead_code)]
    errors: Option<usize>,
    #[serde(rename = "testcase", default)]
    testcases: Vec<JUnitTestCase>,
}

#[derive(Debug, Deserialize)]
struct JUnitTestCase {
    #[serde(rename = "@classname", default)]
    classname: String,
    #[serde(rename = "@name")]
    name: String,
    #[serde(rename = "@time", default)]
    time: f64,
    failure: Option<JUnitFailure>,
}

#[derive(Debug, Deserialize)]
struct JUnitFailure {
    #[serde(rename = "@message", default)]
    message: String,
    #[serde(rename = "$text")]
    #[allow(dead_code)]
    text: Option<String>,
}

// --- Output structs ---

#[derive(Debug, Serialize, Clone)]
pub struct TestRunResult {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub errors: usize,
    pub duration_secs: f64,
    pub tests: Vec<TestCaseResult>,
}

#[derive(Debug, Serialize, Clone)]
pub struct TestCaseResult {
    pub spec: String,
    pub scenario: String,
    pub passed: bool,
    pub duration_secs: f64,
    pub error_message: Option<String>,
}

// --- Failure categorization ---

#[derive(Debug, Serialize, Clone, PartialEq, Eq, Hash)]
pub enum FailureCategory {
    MissingComponent,
    UpgradePrereq,
    UpstreamRegression,
    PlatformIssue,
    ConfigGap,
}

impl std::fmt::Display for FailureCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FailureCategory::MissingComponent => write!(f, "MissingComponent"),
            FailureCategory::UpgradePrereq => write!(f, "UpgradePrereq"),
            FailureCategory::UpstreamRegression => write!(f, "UpstreamRegression"),
            FailureCategory::PlatformIssue => write!(f, "PlatformIssue"),
            FailureCategory::ConfigGap => write!(f, "ConfigGap"),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CategoryGroup {
    pub category: FailureCategory,
    pub count: usize,
    pub tests: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CategorizedTestRunResult {
    #[serde(flatten)]
    pub result: TestRunResult,
    pub categories: Vec<CategoryGroup>,
}

/// Categorize a failure based on error message keywords.
pub fn categorize_failure(error_message: &str) -> FailureCategory {
    let lower = error_message.to_lowercase();

    // MissingComponent: optional components not installed
    if lower.contains("chains") || lower.contains("chain") {
        return FailureCategory::MissingComponent;
    }
    if lower.contains("knative") || lower.contains("kn-apply") || lower.contains("serverless") {
        return FailureCategory::MissingComponent;
    }
    if lower.contains("manualapprovalgate")
        || lower.contains("manual-approval")
        || lower.contains("approval-gate")
        || lower.contains("approvaltask")
    {
        return FailureCategory::MissingComponent;
    }

    // UpgradePrereq: upgrade test prerequisites
    if lower.contains("upgrade")
        && (lower.contains("namespace") || lower.contains("setup") || lower.contains("prerequisite"))
    {
        return FailureCategory::UpgradePrereq;
    }

    // PlatformIssue: platform/OS-level issues
    if lower.contains("uid_map") || (lower.contains("buildah") && lower.contains("namespace")) {
        return FailureCategory::PlatformIssue;
    }

    // ConfigGap: missing configuration/secrets
    if lower.contains("secret") && (lower.contains("missing") || lower.contains("not found")) {
        return FailureCategory::ConfigGap;
    }
    if lower.contains("auth") && (lower.contains("secret") || lower.contains("credential")) {
        return FailureCategory::ConfigGap;
    }

    // Default
    FailureCategory::UpstreamRegression
}

/// Group failed tests by failure category.
pub fn categorize_results(result: &TestRunResult) -> CategorizedTestRunResult {
    let mut groups: std::collections::HashMap<FailureCategory, Vec<String>> =
        std::collections::HashMap::new();

    for test in &result.tests {
        if !test.passed {
            let cat = match &test.error_message {
                Some(msg) => categorize_failure(msg),
                None => FailureCategory::UpstreamRegression,
            };
            groups
                .entry(cat)
                .or_default()
                .push(format!("{}::{}", test.spec, test.scenario));
        }
    }

    let mut categories: Vec<CategoryGroup> = groups
        .into_iter()
        .map(|(category, tests)| CategoryGroup {
            count: tests.len(),
            category,
            tests,
        })
        .collect();

    // Sort by count descending
    categories.sort_by(|a, b| b.count.cmp(&a.count));

    CategorizedTestRunResult {
        result: result.clone(),
        categories,
    }
}

// --- ANSI stripping ---

fn strip_ansi(text: &str) -> String {
    let re = Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    re.replace_all(text, "").to_string()
}

// --- Gauge stdout parser ---

/// Parse Gauge stdout log into structured test results.
/// This is used as a fallback when JUnit XML is not available.
pub fn parse_gauge_stdout(log_path: &Path) -> Result<TestRunResult> {
    let content =
        fs::read_to_string(log_path).with_context(|| format!("Failed to read {}", log_path.display()))?;

    let mut tests: Vec<TestCaseResult> = Vec::new();
    let mut current_spec = String::new();
    let mut current_scenario = String::new();
    let mut scenario_failed = false;
    let mut error_lines: Vec<String> = Vec::new();
    let mut in_error_block = false;
    let mut total_duration: f64 = 0.0;

    for raw_line in content.lines() {
        let line = strip_ansi(raw_line);
        let trimmed = line.trim();

        // Spec header: lines starting with "# "
        if let Some(rest) = trimmed.strip_prefix("# ") {
            // Flush previous scenario if any
            if !current_scenario.is_empty() {
                let error_msg = if scenario_failed && !error_lines.is_empty() {
                    Some(error_lines.join("\n"))
                } else if scenario_failed {
                    Some("Test failed".to_string())
                } else {
                    None
                };
                tests.push(TestCaseResult {
                    spec: current_spec.clone(),
                    scenario: current_scenario.clone(),
                    passed: !scenario_failed,
                    duration_secs: 0.0,
                    error_message: error_msg,
                });
                current_scenario.clear();
                scenario_failed = false;
                error_lines.clear();
                in_error_block = false;
            }
            current_spec = rest.trim().to_string();
            continue;
        }

        // Scenario header: lines starting with "## " (after trimming)
        if let Some(rest) = trimmed.strip_prefix("## ") {
            // Flush previous scenario if any
            if !current_scenario.is_empty() {
                let error_msg = if scenario_failed && !error_lines.is_empty() {
                    Some(error_lines.join("\n"))
                } else if scenario_failed {
                    Some("Test failed".to_string())
                } else {
                    None
                };
                tests.push(TestCaseResult {
                    spec: current_spec.clone(),
                    scenario: current_scenario.clone(),
                    passed: !scenario_failed,
                    duration_secs: 0.0,
                    error_message: error_msg,
                });
            }
            current_scenario = rest.trim().to_string();
            scenario_failed = false;
            error_lines.clear();
            in_error_block = false;
            continue;
        }

        // Step failure detection
        if trimmed.contains("...[FAIL]") {
            scenario_failed = true;
            continue;
        }

        // Error message block
        if trimmed.starts_with("Error Message:") {
            in_error_block = true;
            let msg = trimmed.strip_prefix("Error Message:").unwrap_or("").trim();
            if !msg.is_empty() {
                error_lines.push(msg.to_string());
            }
            continue;
        }

        if in_error_block {
            // End of error block on blank line, stack trace, or next step/scenario
            if trimmed.is_empty()
                || trimmed.starts_with("Stacktrace:")
                || trimmed.starts_with("Failed Step:")
            {
                in_error_block = false;
            } else {
                error_lines.push(trimmed.to_string());
            }
            continue;
        }

        // Duration from final summary line like "FAIL\tcommand-line-arguments\t9487.532s"
        if (trimmed.starts_with("FAIL\t") || trimmed.starts_with("ok\t")) && trimmed.ends_with('s') {
            if let Some(duration_str) = trimmed.rsplit('\t').next() {
                if let Ok(d) = duration_str.trim_end_matches('s').parse::<f64>() {
                    total_duration = d;
                }
            }
        }
    }

    // Flush final scenario
    if !current_scenario.is_empty() {
        let error_msg = if scenario_failed && !error_lines.is_empty() {
            Some(error_lines.join("\n"))
        } else if scenario_failed {
            Some("Test failed".to_string())
        } else {
            None
        };
        tests.push(TestCaseResult {
            spec: current_spec,
            scenario: current_scenario,
            passed: !scenario_failed,
            duration_secs: 0.0,
            error_message: error_msg,
        });
    }

    let passed = tests.iter().filter(|t| t.passed).count();
    let failed = tests.iter().filter(|t| !t.passed).count();

    Ok(TestRunResult {
        total: tests.len(),
        passed,
        failed,
        errors: 0,
        duration_secs: total_duration,
        tests,
    })
}

/// Parse a JUnit XML file into a TestRunResult.
pub fn parse_junit_xml(xml_path: &Path) -> Result<TestRunResult> {
    let xml_content =
        fs::read_to_string(xml_path).with_context(|| format!("Failed to read {}", xml_path.display()))?;

    let suites: JUnitTestSuites =
        quick_xml::de::from_str(&xml_content).context("Failed to parse JUnit XML")?;

    let mut tests = Vec::new();
    let mut total_duration = 0.0;

    for suite in &suites.suites {
        for tc in &suite.testcases {
            let passed = tc.failure.is_none();
            let error_message = tc.failure.as_ref().map(|f| f.message.clone());
            total_duration += tc.time;

            tests.push(TestCaseResult {
                spec: tc.classname.clone(),
                scenario: tc.name.clone(),
                passed,
                duration_secs: tc.time,
                error_message,
            });
        }
    }

    let passed = tests.iter().filter(|t| t.passed).count();
    let failed = tests.iter().filter(|t| !t.passed).count();

    Ok(TestRunResult {
        total: tests.len(),
        passed,
        failed,
        errors: 0,
        duration_secs: total_duration,
        tests,
    })
}

/// Print per-test pass/fail results to stdout with colors.
pub fn print_results(result: &TestRunResult) {
    let green = Style::new().green().bold();
    let red = Style::new().red().bold();

    println!();
    println!("Test Results:");
    println!("{}", "-".repeat(60));

    for test in &result.tests {
        let status = if test.passed {
            green.apply_to("[PASS]")
        } else {
            red.apply_to("[FAIL]")
        };

        println!(
            "{} {}::{} ({:.1}s)",
            status, test.spec, test.scenario, test.duration_secs
        );

        if let Some(ref msg) = test.error_message {
            // Print first line of error only in summary view
            if let Some(first_line) = msg.lines().next() {
                println!("       {}", first_line);
            }
        }
    }

    println!("{}", "-".repeat(60));
    println!(
        "{}/{} passed, {} failed ({:.1}s total)",
        result.passed, result.total, result.failed, result.duration_secs
    );
    println!();
}

/// Print categorized failure analysis after the test results.
pub fn print_categorized_results(categorized: &CategorizedTestRunResult) {
    print_results(&categorized.result);

    if categorized.categories.is_empty() {
        return;
    }

    let yellow = Style::new().yellow().bold();
    let dim = Style::new().dim();

    println!("Failure Analysis:");
    println!("{}", "=".repeat(60));

    for group in &categorized.categories {
        println!(
            "{} ({}):",
            yellow.apply_to(group.category.to_string()),
            group.count
        );
        for test_name in &group.tests {
            println!("  {} {}", dim.apply_to("-"), test_name);
        }
    }

    println!("{}", "=".repeat(60));
    println!();
}

/// Write test results as pretty-printed JSON to a file.
pub fn write_json(result: &TestRunResult, output_path: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(result).context("Failed to serialize results to JSON")?;
    fs::write(output_path, json)
        .with_context(|| format!("Failed to write results to {}", output_path.display()))?;
    Ok(())
}

/// Write categorized test results as pretty-printed JSON.
pub fn write_categorized_json(result: &CategorizedTestRunResult, output_path: &Path) -> Result<()> {
    let json =
        serde_json::to_string_pretty(result).context("Failed to serialize categorized results to JSON")?;
    fs::write(output_path, json)
        .with_context(|| format!("Failed to write results to {}", output_path.display()))?;
    Ok(())
}
