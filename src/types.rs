use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct CheckResult {
    pub name: String,
    pub passed: bool,
    pub detail: String,
    pub fix_hint: Option<String>,
}
