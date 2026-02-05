use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::exec;

/// Clone the openshift-pipelines/operator repo to a temp directory.
pub fn clone_operator_repo(branch: &str) -> Result<PathBuf> {
    let temp_dir = std::env::temp_dir().join(format!("osp-operator-{}", std::process::id()));
    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir)?;
    }
    fs::create_dir_all(&temp_dir)?;

    let url = "https://github.com/openshift-pipelines/operator.git";
    eprintln!("Cloning operator repo (branch: {})...", branch);

    let result = exec::run_cmd(
        "git",
        &["clone", "--depth", "1", "--branch", branch, url, temp_dir.to_str().unwrap()],
    )?;

    if result.exit_code != 0 {
        bail!("Failed to clone operator repo: {}", result.stderr);
    }

    Ok(temp_dir)
}

/// Patch the CSV file with upstream image references.
/// image_map: key = IMAGE_ env var name (e.g., "IMAGE_PIPELINES_CONTROLLER"), value = SHA-pinned pullspec
pub fn patch_csv(operator_dir: &Path, image_map: &HashMap<String, String>) -> Result<()> {
    let csv_path = operator_dir
        .join(".konflux/olm-catalog/bundle/manifests/openshift-pipelines-operator-rh.clusterserviceversion.yaml");

    if !csv_path.exists() {
        bail!("CSV file not found at {}", csv_path.display());
    }

    eprintln!("Patching CSV with {} upstream images...", image_map.len());

    let content = fs::read_to_string(&csv_path)
        .context("Failed to read CSV file")?;

    let mut doc: serde_yaml::Value = serde_yaml::from_str(&content)
        .context("Failed to parse CSV YAML")?;

    // Navigate to spec.install.spec.deployments[*].spec.template.spec.containers[*].env
    patch_env_vars_recursive(&mut doc, image_map);

    let patched = serde_yaml::to_string(&doc)?;
    fs::write(&csv_path, patched)?;

    eprintln!("  Patched CSV at {}", csv_path.display());
    Ok(())
}

fn patch_env_vars_recursive(value: &mut serde_yaml::Value, image_map: &HashMap<String, String>) {
    match value {
        serde_yaml::Value::Mapping(map) => {
            // Check if this is an env var entry with name/value
            let name_key = serde_yaml::Value::String("name".to_string());
            let value_key = serde_yaml::Value::String("value".to_string());

            // First, check if we need to patch this entry
            let should_patch = if let Some(serde_yaml::Value::String(name)) = map.get(&name_key) {
                if name.starts_with("IMAGE_") {
                    image_map.get(name).cloned()
                } else {
                    None
                }
            } else {
                None
            };

            // Apply the patch if needed
            if let Some(new_val) = should_patch {
                if let Some(val) = map.get_mut(&value_key) {
                    *val = serde_yaml::Value::String(new_val);
                }
            }

            // Recurse into all values
            for (_, v) in map.iter_mut() {
                patch_env_vars_recursive(v, image_map);
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            for item in seq.iter_mut() {
                patch_env_vars_recursive(item, image_map);
            }
        }
        _ => {}
    }
}

/// Build the operator bundle image and push to registry.
/// Returns the SHA-pinned pullspec.
pub fn build_bundle_image(operator_dir: &Path, registry: &str, tag: &str) -> Result<String> {
    let bundle_dir = operator_dir.join(".konflux/olm-catalog/bundle");
    let dockerfile = bundle_dir.join("bundle.Dockerfile");

    if !dockerfile.exists() {
        bail!("Bundle Dockerfile not found at {}", dockerfile.display());
    }

    let image_ref = format!("{}/osp-upstream-bundle:{}", registry, tag);
    eprintln!("Building bundle image: {}", image_ref);

    // Build with buildah
    let result = exec::run_cmd(
        "buildah",
        &[
            "build",
            "-f", dockerfile.to_str().unwrap(),
            "-t", &image_ref,
            bundle_dir.to_str().unwrap(),
        ],
    )?;

    if result.exit_code != 0 {
        bail!("Failed to build bundle image: {}", result.stderr);
    }

    // Validate bundle
    eprintln!("Validating bundle...");
    let validate_result = exec::run_cmd(
        "opm",
        &["render", &image_ref],
    );

    if let Ok(r) = validate_result {
        if r.exit_code != 0 {
            eprintln!("WARNING: Bundle validation with opm render failed, continuing anyway");
        }
    }

    // Push
    eprintln!("Pushing bundle image...");
    let push_result = exec::run_cmd(
        "buildah",
        &["push", &image_ref],
    )?;

    if push_result.exit_code != 0 {
        bail!("Failed to push bundle image: {}", push_result.stderr);
    }

    // Get digest
    let digest = get_image_digest(&image_ref)?;
    let sha_ref = format!("{}@{}", image_ref.split(':').next().unwrap(), digest);

    eprintln!("  Bundle pushed: {}", sha_ref);
    Ok(sha_ref)
}

/// Build the FBC index image containing the bundle.
/// Returns the SHA-pinned pullspec.
pub fn build_index_image(bundle_pullspec: &str, registry: &str, tag: &str) -> Result<String> {
    let temp_dir = std::env::temp_dir().join(format!("fbc-index-{}", std::process::id()));
    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir)?;
    }
    fs::create_dir_all(&temp_dir)?;

    let catalog_dir = temp_dir.join("catalog");
    fs::create_dir_all(&catalog_dir)?;

    eprintln!("Rendering bundle to FBC catalog...");

    // Render bundle to catalog.yaml
    let render_result = exec::run_cmd(
        "opm",
        &["render", bundle_pullspec, "-o", "yaml"],
    )?;

    if render_result.exit_code != 0 {
        bail!("Failed to render bundle: {}", render_result.stderr);
    }

    // Write catalog.yaml with rendered content + channel entry
    let mut catalog_content = render_result.stdout.clone();

    // Add channel entry
    catalog_content.push_str("\n---\n");
    catalog_content.push_str("schema: olm.channel\n");
    catalog_content.push_str("package: openshift-pipelines-operator-rh\n");
    catalog_content.push_str("name: upstream-testing\n");
    catalog_content.push_str("entries:\n");
    catalog_content.push_str("  - name: openshift-pipelines-operator-rh.v99.0.0-upstream\n");

    fs::write(catalog_dir.join("catalog.yaml"), &catalog_content)?;

    // Validate catalog
    eprintln!("Validating FBC catalog...");
    let validate_result = exec::run_cmd(
        "opm",
        &["validate", catalog_dir.to_str().unwrap()],
    )?;

    if validate_result.exit_code != 0 {
        bail!("FBC catalog validation failed: {}", validate_result.stderr);
    }

    // Create Dockerfile
    let dockerfile_content = r#"FROM registry.redhat.io/openshift4/ose-operator-registry:v4.17
COPY catalog /configs
LABEL operators.operatorframework.io.index.configs.v1=/configs
"#;
    fs::write(temp_dir.join("Dockerfile"), dockerfile_content)?;

    // Copy catalog to build context
    let build_catalog_dir = temp_dir.join("catalog");
    // Already exists from above

    let image_ref = format!("{}/osp-upstream-index:{}", registry, tag);
    eprintln!("Building FBC index image: {}", image_ref);

    // Build with buildah
    let result = exec::run_cmd(
        "buildah",
        &[
            "build",
            "-f", temp_dir.join("Dockerfile").to_str().unwrap(),
            "-t", &image_ref,
            temp_dir.to_str().unwrap(),
        ],
    )?;

    if result.exit_code != 0 {
        bail!("Failed to build index image: {}", result.stderr);
    }

    // Push
    eprintln!("Pushing FBC index image...");
    let push_result = exec::run_cmd(
        "buildah",
        &["push", &image_ref],
    )?;

    if push_result.exit_code != 0 {
        bail!("Failed to push index image: {}", push_result.stderr);
    }

    // Get digest
    let digest = get_image_digest(&image_ref)?;
    let sha_ref = format!("{}@{}", image_ref.split(':').next().unwrap(), digest);

    // Cleanup
    let _ = fs::remove_dir_all(&temp_dir);

    eprintln!("  Index pushed: {}", sha_ref);
    Ok(sha_ref)
}

fn get_image_digest(image_ref: &str) -> Result<String> {
    let result = exec::run_cmd(
        "skopeo",
        &["inspect", "--format", "{{.Digest}}", &format!("docker://{}", image_ref)],
    )?;

    if result.exit_code != 0 {
        bail!("Failed to get image digest: {}", result.stderr);
    }

    Ok(result.stdout.trim().to_string())
}
