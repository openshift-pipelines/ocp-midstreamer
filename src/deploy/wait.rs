use anyhow::{bail, Context};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, ApiResource, DynamicObject, ListParams};
use kube::Client;
use tokio::runtime::Runtime;

use crate::progress;

/// Known namespaces where Tekton pods may run.
const TEKTON_NAMESPACES: &[&str] = &["openshift-pipelines", "tekton-pipelines"];

/// Wait for operator reconciliation: TektonConfig Ready=True and pod images matching expected.
///
/// Uses exponential backoff: start 5s, double each time, cap at 60s, max 10 retries.
pub fn wait_for_reconciliation(
    rt: &Runtime,
    client: &Client,
    expected_images: &[(String, String)],
    verbose: bool,
) -> anyhow::Result<()> {
    let max_retries: u32 = 20;
    let mut delay_secs: u64 = 10;
    let cap_secs: u64 = 30;

    let pb = progress::stage_spinner("Waiting for operator reconciliation...");

    for attempt in 1..=max_retries {
        pb.set_message(format!(
            "Retry {}/{}: checking reconciliation status...",
            attempt, max_retries
        ));
        eprintln!(
            "  Retry {}/{}: checking reconciliation status...",
            attempt, max_retries
        );

        let config_ready = check_tektonconfig_ready(rt, client, verbose)?;
        let images_ok = if config_ready {
            verify_pod_images(rt, client, expected_images, verbose)?
        } else {
            if verbose {
                eprintln!("    TektonConfig not yet Ready");
            }
            false
        };

        if config_ready && images_ok {
            progress::finish_spinner(&pb, true);
            eprintln!("  All Tekton pods reconciled with upstream images.");
            return Ok(());
        }

        if attempt < max_retries {
            std::thread::sleep(std::time::Duration::from_secs(delay_secs));
            delay_secs = (delay_secs * 2).min(cap_secs);
        }
    }

    progress::finish_spinner(&pb, false);

    // Build failure message
    let config_ready = check_tektonconfig_ready(rt, client, false).unwrap_or(false);
    let mut msg = String::from("Reconciliation did not converge after max retries.\n");
    if !config_ready {
        msg.push_str("  - TektonConfig is NOT Ready\n");
    }
    msg.push_str("  - Some pod images may not match expected upstream images\n");
    msg.push_str("  Suggestion: check operator logs with:\n");
    msg.push_str("    oc logs -n openshift-pipelines deploy/openshift-pipelines-operator\n");

    bail!("{}", msg)
}

/// Check if TektonConfig "config" has condition Ready=True.
fn check_tektonconfig_ready(
    rt: &Runtime,
    client: &Client,
    verbose: bool,
) -> anyhow::Result<bool> {
    let ar = ApiResource {
        group: "operator.tekton.dev".into(),
        version: "v1alpha1".into(),
        api_version: "operator.tekton.dev/v1alpha1".into(),
        kind: "TektonConfig".into(),
        plural: "tektonconfigs".into(),
    };

    let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);
    let tc = rt
        .block_on(api.get("config"))
        .context("Failed to get TektonConfig 'config'")?;

    // Parse status.conditions from the dynamic object
    if let Some(status) = tc.data.get("status") {
        if let Some(conditions) = status.get("conditions") {
            if let Some(conditions_arr) = conditions.as_array() {
                for cond in conditions_arr {
                    let ctype = cond.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let cstatus = cond.get("status").and_then(|v| v.as_str()).unwrap_or("");
                    if verbose {
                        eprintln!("    TektonConfig condition: {}={}", ctype, cstatus);
                    }
                    if ctype == "Ready" && cstatus == "True" {
                        return Ok(true);
                    }
                }
            }
        }
    }

    Ok(false)
}

/// Verify that running pods in Tekton namespaces have the expected images.
///
/// For each expected image ref, checks if any pod container image matches.
/// This is best-effort: not all IMAGE_ env vars map 1:1 to pod containers.
fn verify_pod_images(
    rt: &Runtime,
    client: &Client,
    expected_images: &[(String, String)],
    verbose: bool,
) -> anyhow::Result<bool> {
    // Collect all container images from Tekton pods
    let mut found_images: Vec<String> = Vec::new();

    for ns in TEKTON_NAMESPACES {
        let api: Api<Pod> = Api::namespaced(client.clone(), ns);
        let lp = ListParams::default().labels("app.kubernetes.io/part-of=tekton-pipelines");
        let pods = rt
            .block_on(api.list(&lp))
            .unwrap_or_else(|_| kube::api::ObjectList {
                metadata: Default::default(),
                items: vec![],
                types: Default::default(),
            });

        for pod in &pods.items {
            if let Some(spec) = &pod.spec {
                for container in &spec.containers {
                    if let Some(image) = &container.image {
                        found_images.push(image.clone());
                    }
                }
            }
        }
    }

    if verbose {
        eprintln!("    Found {} container images in Tekton pods", found_images.len());
    }

    // Check each expected image ref appears in at least one pod
    let mut all_found = true;
    for (_env_key, image_ref) in expected_images {
        let matched = found_images.iter().any(|img| img.contains(image_ref) || image_ref.contains(img));
        if !matched {
            if verbose {
                eprintln!("    Missing expected image: {}", image_ref);
            }
            all_found = false;
        }
    }

    Ok(all_found)
}
