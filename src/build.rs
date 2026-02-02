use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};
use tokio::task::JoinSet;

use crate::component::{self, ComponentSpec};
use crate::config::ComponentConfig;
use crate::exec;
use crate::progress;
use crate::registry;

/// Clone a git repository (shallow, depth 1) into the given destination directory.
pub fn clone_repo(repo_url: &str, dest: &Path) -> Result<()> {
    let dest_str = dest.to_str().unwrap_or_default();
    exec::run_cmd("git", &["clone", "--depth", "1", repo_url, dest_str])?;
    Ok(())
}

/// Build images using ko with streaming output.
///
/// Sets `KO_DOCKER_REPO` and `GOFLAGS=-mod=vendor` env vars.
/// Uses `--base-import-paths` so image names match the last path segment.
/// Runs ko from `source_dir` with `current_dir`.
/// Returns the list of expected image names derived from import paths.
pub fn ko_build(source_dir: &Path, registry: &str, import_paths: &[String]) -> Result<Vec<String>> {
    ko_build_with_external(source_dir, registry, import_paths, None)
}

/// Build images using ko, optionally pushing to an external registry.
///
/// When `external_registry` is Some, images are copied from the build registry
/// to the external registry using skopeo, and SHA-pinned external pullspecs are returned.
/// When None, behaves like the original ko_build (returns image names only).
pub fn ko_build_with_external(
    source_dir: &Path,
    registry: &str,
    import_paths: &[String],
    external_registry: Option<&str>,
) -> Result<Vec<String>> {
    // Create a temp file for --image-refs output
    let image_refs_file = source_dir.join(".ko-image-refs");

    let image_refs_path_str = image_refs_file.to_string_lossy().to_string();
    let mut args: Vec<&str> = vec!["build", "--base-import-paths", "--sbom=none", "--image-refs", &image_refs_path_str];
    for p in import_paths {
        args.push(p.as_str());
    }

    let docker_config = std::env::var("DOCKER_CONFIG")
        .unwrap_or_else(|_| String::new());
    let mut envs: Vec<(&str, &str)> = vec![
        ("KO_DOCKER_REPO", registry),
        ("GOFLAGS", "-mod=vendor"),
    ];
    if !docker_config.is_empty() {
        envs.push(("DOCKER_CONFIG", &docker_config));
    }

    let status = Command::new("ko")
        .args(&args)
        .envs(envs.iter().cloned())
        .current_dir(source_dir)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| "failed to execute ko")?;

    let code = status.code().unwrap_or(-1);
    if code != 0 {
        anyhow::bail!("ko build failed with exit code {}", code);
    }

    // Collect SHA-pinned image refs from ko output
    let image_refs = if image_refs_file.exists() {
        registry::collect_image_refs(&image_refs_file)?
    } else {
        // Fallback: derive names from import paths (no SHA info)
        import_paths
            .iter()
            .filter_map(|p| p.rsplit('/').next())
            .map(|s| (s.to_string(), format!("{}/{}", registry, s)))
            .collect()
    };

    // If external registry is specified, push each image there
    if let Some(ext_registry) = external_registry {
        eprintln!("Pushing {} images to external registry: {}", image_refs.len(), ext_registry);
        let mut external_pullspecs = Vec::new();
        for (_short_name, sha_ref) in &image_refs {
            let pinned = registry::push_to_external(sha_ref, ext_registry)?;
            external_pullspecs.push(pinned);
        }
        return Ok(external_pullspecs);
    }

    // Return image names (original behavior)
    let image_names: Vec<String> = image_refs
        .iter()
        .map(|(short_name, _)| short_name.clone())
        .collect();

    Ok(image_names)
}

/// Build images using docker/podman for non-ko components (e.g. console-plugin).
///
/// Tries podman first, falls back to docker.
/// Builds and pushes each image defined in the config images map.
pub fn docker_build(source_dir: &Path, registry: &str, images: &HashMap<String, String>) -> Result<Vec<String>> {
    let mut built = Vec::new();
    for image_name in images.keys() {
        let tag = format!("{}/{}", registry, image_name);
        // Try podman first, fall back to docker
        let builder = if Command::new("podman").arg("--version").output().is_ok() {
            "podman"
        } else {
            "docker"
        };
        let status = Command::new(builder)
            .args(["build", "-t", &tag, "."])
            .current_dir(source_dir)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .with_context(|| format!("failed to execute {builder} build"))?;
        if !status.success() {
            anyhow::bail!("{builder} build failed for {image_name}");
        }
        // Push
        let push_status = Command::new(builder)
            .args(["push", &tag, "--tls-verify=false"])
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .with_context(|| format!("failed to push {image_name}"))?;
        if !push_status.success() {
            anyhow::bail!("{builder} push failed for {image_name}");
        }
        built.push(image_name.clone());
    }
    Ok(built)
}

/// Build multiple components in parallel using tokio JoinSet.
///
/// Each component gets its own spinner via MultiProgress.
/// Failed builds do not block other builds.
/// Returns a Vec of (component_name, Result<image_names>).
pub async fn build_components_parallel(
    specs: &[ComponentSpec],
    configs: &HashMap<String, ComponentConfig>,
    registry: &str,
) -> Vec<(String, Result<Vec<String>>)> {
    let mp = progress::multi_progress();
    let mut set = JoinSet::new();

    for spec in specs {
        let comp_name = spec.name.clone();
        let git_ref = spec.git_ref.clone();
        let pb = progress::component_spinner(&mp, &comp_name);

        let comp_cfg = match configs.get(&comp_name) {
            Some(c) => c,
            None => {
                pb.finish_with_message(format!("{comp_name}: FAILED - not in config"));
                // We can't spawn a task, just push the error result later
                // Use a trivial task that returns the error
                set.spawn(async move {
                    (comp_name, Err(anyhow::anyhow!("component not in config")))
                });
                continue;
            }
        };

        let repo_url = comp_cfg.repo.clone();
        let import_paths = comp_cfg.import_paths.clone();
        let build_system = comp_cfg.build_system.clone();
        let images = comp_cfg.images.clone();
        let registry = registry.to_string();

        set.spawn(async move {
            // Clone
            pb.set_message(format!("{comp_name}: cloning..."));
            let temp_dir = match tempfile::tempdir() {
                Ok(d) => d,
                Err(e) => {
                    let msg = format!("{comp_name}: FAILED - {e}");
                    pb.finish_with_message(msg);
                    return (comp_name, Err(anyhow::anyhow!("temp dir: {e}")));
                }
            };

            let clone_dest = temp_dir.path().to_path_buf();
            let clone_repo = repo_url.clone();
            let clone_ref = git_ref.clone();
            let clone_result = tokio::task::spawn_blocking(move || {
                component::clone_with_ref(&clone_repo, &clone_dest, clone_ref.as_deref())
            })
            .await;

            match clone_result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    pb.finish_with_message(format!("{comp_name}: FAILED - {e}"));
                    return (comp_name, Err(e));
                }
                Err(e) => {
                    pb.finish_with_message(format!("{comp_name}: FAILED - join error"));
                    return (comp_name, Err(anyhow::anyhow!("join error: {e}")));
                }
            }

            // Build
            pb.set_message(format!("{comp_name}: building..."));
            let build_dir = temp_dir.path().to_path_buf();
            let build_registry = registry.clone();
            let build_paths = import_paths.clone();
            let build_images = images.clone();
            let build_sys = build_system.clone();
            let build_result = tokio::task::spawn_blocking(move || {
                match build_sys.as_deref() {
                    Some("docker") => docker_build(&build_dir, &build_registry, &build_images),
                    _ => ko_build(&build_dir, &build_registry, &build_paths),
                }
            })
            .await;

            match build_result {
                Ok(Ok(images)) => {
                    pb.finish_with_message(format!("{comp_name}: done ({} images)", images.len()));
                    (comp_name, Ok(images))
                }
                Ok(Err(e)) => {
                    pb.finish_with_message(format!("{comp_name}: FAILED - {e}"));
                    (comp_name, Err(e))
                }
                Err(e) => {
                    pb.finish_with_message(format!("{comp_name}: FAILED - join error"));
                    (comp_name, Err(anyhow::anyhow!("join error: {e}")))
                }
            }
        });
    }

    let mut results = Vec::new();
    while let Some(res) = set.join_next().await {
        match res {
            Ok(pair) => results.push(pair),
            Err(e) => results.push(("unknown".to_string(), Err(anyhow::anyhow!("task panic: {e}")))),
        }
    }

    results
}
