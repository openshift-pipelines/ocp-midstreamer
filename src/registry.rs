use anyhow::{Context, Result};
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

/// Push an image to an external registry (e.g. quay.io) using skopeo.
///
/// Copies the image from its current location to the target registry.
/// Returns the SHA-pinned pullspec (e.g. quay.io/org/image@sha256:abc...).
pub fn push_to_external(image_ref: &str, target_registry: &str) -> Result<String> {
    // Derive image name from the source ref (last path segment before @/: tag)
    let image_name = image_ref
        .rsplit('/')
        .next()
        .unwrap_or(image_ref)
        .split('@')
        .next()
        .unwrap_or(image_ref)
        .split(':')
        .next()
        .unwrap_or(image_ref);

    let dest = format!("docker://{}/{}", target_registry, image_name);
    let src = format!("docker://{}", image_ref);

    let _result = exec::run_cmd(
        "skopeo",
        &["copy", "--all", &src, &dest],
    )?;

    // Get the digest of the pushed image via skopeo inspect
    let inspect_result = exec::run_cmd(
        "skopeo",
        &["inspect", "--format", "{{.Digest}}", &dest],
    )?;
    let digest = inspect_result.stdout.trim().to_string();

    let pinned = format!("{}/{}@{}", target_registry, image_name, digest);
    eprintln!("  Pushed: {}", pinned);
    Ok(pinned)
}

/// Collect image references from ko's --image-refs output file.
///
/// Each line in the file is a SHA-pinned image reference produced by ko.
/// Returns Vec of (short_name, sha_pullspec) pairs where short_name is the
/// last path segment before the @sha256: digest.
pub fn collect_image_refs(image_refs_file: &std::path::Path) -> Result<Vec<(String, String)>> {
    let content = std::fs::read_to_string(image_refs_file)
        .with_context(|| format!("failed to read image refs file: {}", image_refs_file.display()))?;

    let mut refs = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Extract short name: last path segment before @sha256:
        let short_name = line
            .rsplit('/')
            .next()
            .unwrap_or(line)
            .split('@')
            .next()
            .unwrap_or(line)
            .to_string();
        refs.push((short_name, line.to_string()));
    }
    Ok(refs)
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
