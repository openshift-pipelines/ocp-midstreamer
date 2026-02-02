use anyhow::Result;
use std::fs;
use std::path::PathBuf;

use crate::exec;

/// Default namespace for upstream Tekton deployments.
pub const DEFAULT_NAMESPACE: &str = "tekton-upstream";

/// Get the OCP internal image registry route.
///
/// Queries the `default-route` in the `openshift-image-registry` namespace.
pub fn get_registry_route() -> Result<String> {
    let result = exec::run_cmd(
        "oc",
        &[
            "get", "route", "default-route",
            "-n", "openshift-image-registry",
            "-o", "jsonpath={.spec.host}",
        ],
    );

    match result {
        Ok(r) => Ok(r.stdout.trim().to_string()),
        Err(_) => anyhow::bail!(
            "Could not get OCP image registry route. Is the registry exposed? \
             Run: oc patch configs.imageregistry.operator.openshift.io/cluster \
             --patch '{{\"spec\":{{\"defaultRoute\":true}}}}' --type=merge"
        ),
    }
}

/// Authenticate to the OCP internal registry using the current oc token.
pub fn registry_login(registry_route: &str) -> Result<()> {
    let token_result = exec::run_cmd("oc", &["whoami", "-t"])?;
    let token = token_result.stdout.trim();

    let registry_arg = format!("--registry={}", registry_route);
    let token_arg = format!("--token={}", token);

    exec::run_cmd(
        "oc",
        &["registry", "login", &registry_arg, &token_arg, "--insecure=true"],
    )?;

    // ko reads Docker credentials from ~/.docker/config.json, but oc registry login
    // writes to ~/.config/containers/auth.json. Copy credentials so ko can push.
    sync_docker_config();

    Ok(())
}

/// Copy container auth credentials to ~/.docker/config.json for ko compatibility.
///
/// oc registry login writes to ~/.config/containers/auth.json (podman convention).
/// ko uses the Docker convention (~/.docker/config.json). This function merges
/// the container auth into the Docker config so ko can authenticate to the registry.
fn sync_docker_config() {
    let home = match std::env::var("HOME") {
        Ok(h) => PathBuf::from(h),
        Err(_) => return,
    };

    let containers_auth = home.join(".config/containers/auth.json");
    let docker_dir = home.join(".docker");
    let docker_config = docker_dir.join("config.json");

    if !containers_auth.exists() {
        return;
    }

    // Read source credentials
    let src = match fs::read_to_string(&containers_auth) {
        Ok(s) => s,
        Err(_) => return,
    };

    let _ = fs::create_dir_all(&docker_dir);

    if docker_config.exists() {
        // Merge: parse both, merge auths from containers into docker config
        let src_json: serde_json::Value = match serde_json::from_str(&src) {
            Ok(v) => v,
            Err(_) => return,
        };
        let existing = match fs::read_to_string(&docker_config) {
            Ok(s) => s,
            Err(_) => return,
        };
        let mut dst_json: serde_json::Value = match serde_json::from_str(&existing) {
            Ok(v) => v,
            Err(_) => {
                // Existing file is invalid JSON, overwrite
                let _ = fs::write(&docker_config, &src);
                return;
            }
        };

        if let (Some(src_auths), Some(dst_auths)) = (
            src_json.get("auths").and_then(|a| a.as_object()),
            dst_json.get_mut("auths").and_then(|a| a.as_object_mut()),
        ) {
            for (k, v) in src_auths {
                dst_auths.insert(k.clone(), v.clone());
            }
        }

        let _ = fs::write(&docker_config, serde_json::to_string_pretty(&dst_json).unwrap_or_default());
    } else {
        // No existing docker config, just copy
        let _ = fs::write(&docker_config, &src);
    }
}

/// Ensure a namespace exists, creating it if necessary.
pub fn ensure_namespace(namespace: &str) -> Result<()> {
    let check = exec::run_cmd_unchecked("oc", &["get", "namespace", namespace])?;
    if check.exit_code == 0 {
        return Ok(());
    }

    exec::run_cmd("oc", &["create", "namespace", namespace])?;
    Ok(())
}
