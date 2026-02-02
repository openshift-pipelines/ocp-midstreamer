use anyhow::{bail, Context};
use k8s_openapi::api::core::v1::Namespace;
use kube::api::{Api, ApiResource, DynamicObject, Patch, PatchParams, PostParams};
use kube::Client;
use serde_json::json;
use tokio::runtime::Runtime;

use crate::{exec, progress, registry};

/// Run all auto-setup steps with partial-failure continuation.
/// Each step is attempted independently; failures are warned but do not abort.
pub fn run_auto_setup() -> anyhow::Result<()> {
    let (rt, client) = crate::k8s::create_kube_client()?;

    let mut warnings: Vec<String> = Vec::new();

    // Step 1: Ensure image registry route
    {
        let pb = progress::stage_spinner("Ensuring image registry route");
        if let Err(e) = ensure_registry_route(&rt, &client) {
            let msg = format!("Registry route setup: {e:#}");
            eprintln!("WARNING: {msg}");
            warnings.push(msg);
            progress::finish_spinner(&pb, false);
        } else {
            progress::finish_spinner(&pb, true);
        }
    }

    // Step 2: Wait for registry route
    {
        let pb = progress::stage_spinner("Waiting for registry route");
        if let Err(e) = wait_for_registry_route(&rt, &client) {
            let msg = format!("Registry route wait: {e:#}");
            eprintln!("WARNING: {msg}");
            warnings.push(msg);
            progress::finish_spinner(&pb, false);
        } else {
            progress::finish_spinner(&pb, true);
        }
    }

    // Step 3: Ensure namespace and RBAC
    {
        let pb = progress::stage_spinner("Ensuring namespace and RBAC");
        if let Err(e) = ensure_namespace_rbac(&rt, &client) {
            let msg = format!("Namespace/RBAC setup: {e:#}");
            eprintln!("WARNING: {msg}");
            warnings.push(msg);
            progress::finish_spinner(&pb, false);
        } else {
            progress::finish_spinner(&pb, true);
        }
    }

    // Step 4: Ensure operator installed
    {
        let pb = progress::stage_spinner("Ensuring OpenShift Pipelines operator");
        if let Err(e) = ensure_operator_installed(&rt, &client) {
            let msg = format!("Operator install: {e:#}");
            eprintln!("WARNING: {msg}");
            warnings.push(msg);
            progress::finish_spinner(&pb, false);
        } else {
            progress::finish_spinner(&pb, true);
        }
    }

    // Step 5: Wait for operator ready
    {
        let pb = progress::stage_spinner("Waiting for operator ready (up to 5 min)");
        if let Err(e) = wait_for_operator_ready(&rt, &client) {
            let msg = format!("Operator ready wait: {e:#}");
            eprintln!("WARNING: {msg}");
            warnings.push(msg);
            progress::finish_spinner(&pb, false);
        } else {
            progress::finish_spinner(&pb, true);
        }
    }

    // Step 6: Ensure TektonConfig
    {
        let pb = progress::stage_spinner("Ensuring TektonConfig CR");
        if let Err(e) = ensure_tektonconfig(&rt, &client) {
            let msg = format!("TektonConfig setup: {e:#}");
            eprintln!("WARNING: {msg}");
            warnings.push(msg);
            progress::finish_spinner(&pb, false);
        } else {
            progress::finish_spinner(&pb, true);
        }
    }

    if !warnings.is_empty() {
        eprintln!("\nAuto-setup completed with {} warning(s):", warnings.len());
        for w in &warnings {
            eprintln!("  - {w}");
        }
    } else {
        eprintln!("\nAuto-setup completed successfully.");
    }

    Ok(())
}

/// Ensure the internal image registry is configured with a default route.
/// Patches the image-registry config to Managed state, enables defaultRoute,
/// and sets emptyDir storage if no storage is configured.
pub fn ensure_registry_route(rt: &Runtime, client: &Client) -> anyhow::Result<()> {
    let ar = ApiResource {
        group: "imageregistry.operator.openshift.io".into(),
        version: "v1".into(),
        api_version: "imageregistry.operator.openshift.io/v1".into(),
        kind: "Config".into(),
        plural: "configs".into(),
    };

    let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);
    let config = rt
        .block_on(api.get("cluster"))
        .context("Failed to get image registry config")?;

    let spec = config.data.get("spec");

    let mgmt_state = spec
        .and_then(|s| s.get("managementState"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    let default_route = spec
        .and_then(|s| s.get("defaultRoute"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let storage_empty = spec
        .and_then(|s| s.get("storage"))
        .map(|v| v.is_null() || (v.is_object() && v.as_object().unwrap().is_empty()))
        .unwrap_or(true);

    let mut patch = serde_json::Map::new();
    let mut spec_patch = serde_json::Map::new();

    if mgmt_state == "Removed" {
        spec_patch.insert("managementState".into(), json!("Managed"));
    }
    if !default_route {
        spec_patch.insert("defaultRoute".into(), json!(true));
    }
    if storage_empty {
        spec_patch.insert("storage".into(), json!({"emptyDir": {}}));
    }

    if spec_patch.is_empty() {
        eprintln!("  Image registry already configured.");
        return Ok(());
    }

    patch.insert("spec".into(), json!(spec_patch));

    rt.block_on(api.patch(
        "cluster",
        &PatchParams::default(),
        &Patch::Merge(json!(patch)),
    ))
    .context("Failed to patch image registry config")?;

    eprintln!("  Patched image registry config.");
    Ok(())
}

/// Wait for the default-route Route to appear in openshift-image-registry.
/// Polls for up to 30 seconds.
pub fn wait_for_registry_route(_rt: &Runtime, _client: &Client) -> anyhow::Result<()> {
    let timeout = std::time::Duration::from_secs(30);
    let interval = std::time::Duration::from_secs(2);
    let start = std::time::Instant::now();

    loop {
        let result = exec::run_cmd_unchecked(
            "oc",
            &[
                "get",
                "route",
                "default-route",
                "-n",
                "openshift-image-registry",
                "-o",
                "name",
            ],
        )?;

        if result.exit_code == 0 && result.stdout.contains("route") {
            eprintln!("  Registry route is available.");
            return Ok(());
        }

        if start.elapsed() > timeout {
            bail!("Timed out waiting for registry route (30s)");
        }

        std::thread::sleep(interval);
    }
}

/// Ensure the image namespace exists and has image-puller RBAC for all authenticated users.
pub fn ensure_namespace_rbac(rt: &Runtime, client: &Client) -> anyhow::Result<()> {
    let ns_name = registry::DEFAULT_NAMESPACE;

    // Create namespace if it doesn't exist
    let ns_api: Api<Namespace> = Api::all(client.clone());
    match rt.block_on(ns_api.get(ns_name)) {
        Ok(_) => {
            eprintln!("  Namespace {ns_name} already exists.");
        }
        Err(kube::Error::Api(resp)) if resp.code == 404 => {
            let ns: Namespace = serde_json::from_value(json!({
                "apiVersion": "v1",
                "kind": "Namespace",
                "metadata": {
                    "name": ns_name
                }
            }))?;
            rt.block_on(ns_api.create(&PostParams::default(), &ns))
                .with_context(|| format!("Failed to create namespace {ns_name}"))?;
            eprintln!("  Created namespace {ns_name}.");
        }
        Err(e) => return Err(e).context(format!("Failed to check namespace {ns_name}")),
    }

    // Ensure image-puller RBAC (reuses pattern from deploy/operator.rs)
    crate::deploy::operator::ensure_image_pull_rbac(rt, client, ns_name)?;

    Ok(())
}

/// Ensure the OpenShift Pipelines operator is installed via OLM Subscription.
/// If TektonConfig already exists, the operator is already installed — skip.
pub fn ensure_operator_installed(rt: &Runtime, client: &Client) -> anyhow::Result<()> {
    // Check if TektonConfig already exists (operator fully installed)
    let tc_ar = ApiResource {
        group: "operator.tekton.dev".into(),
        version: "v1alpha1".into(),
        api_version: "operator.tekton.dev/v1alpha1".into(),
        kind: "TektonConfig".into(),
        plural: "tektonconfigs".into(),
    };
    let tc_api: Api<DynamicObject> = Api::all_with(client.clone(), &tc_ar);
    if rt.block_on(tc_api.get("config")).is_ok() {
        eprintln!("  TektonConfig already exists — operator is installed.");
        return Ok(());
    }

    // Check if Subscription already exists
    let sub_ar = ApiResource {
        group: "operators.coreos.com".into(),
        version: "v1alpha1".into(),
        api_version: "operators.coreos.com/v1alpha1".into(),
        kind: "Subscription".into(),
        plural: "subscriptions".into(),
    };
    let sub_api: Api<DynamicObject> =
        Api::namespaced_with(client.clone(), "openshift-operators", &sub_ar);

    if rt.block_on(sub_api.get("openshift-pipelines-operator")).is_ok() {
        eprintln!("  Subscription already exists — waiting for operator.");
        return Ok(());
    }

    // Create Subscription
    let sub: DynamicObject = serde_json::from_value(json!({
        "apiVersion": "operators.coreos.com/v1alpha1",
        "kind": "Subscription",
        "metadata": {
            "name": "openshift-pipelines-operator",
            "namespace": "openshift-operators"
        },
        "spec": {
            "channel": "latest",
            "name": "openshift-pipelines-operator-rh",
            "source": "redhat-operators",
            "sourceNamespace": "openshift-marketplace",
            "installPlanApproval": "Automatic"
        }
    }))?;

    rt.block_on(sub_api.create(&PostParams::default(), &sub))
        .context("Failed to create OpenShift Pipelines operator Subscription")?;

    eprintln!("  Created operator Subscription.");
    Ok(())
}

/// Wait for the operator deployment to become Available.
/// Checks known namespaces and deployment names for up to 5 minutes.
pub fn wait_for_operator_ready(rt: &Runtime, client: &Client) -> anyhow::Result<()> {
    use k8s_openapi::api::apps::v1::Deployment;

    let timeout = std::time::Duration::from_secs(300);
    let interval = std::time::Duration::from_secs(5);
    let start = std::time::Instant::now();

    let namespaces = &[
        "openshift-pipelines",
        "openshift-operators",
        "tekton-pipelines",
    ];
    let deployment_names = &["openshift-pipelines-operator", "tekton-operator"];

    loop {
        for ns in namespaces {
            let api: Api<Deployment> = Api::namespaced(client.clone(), ns);
            for name in deployment_names {
                if let Ok(dep) = rt.block_on(api.get(name)) {
                    if let Some(status) = &dep.status {
                        if let Some(conditions) = &status.conditions {
                            for cond in conditions {
                                if cond.type_ == "Available" && cond.status == "True" {
                                    eprintln!("  Operator deployment {name} is Available in {ns}.");
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
            }
        }

        if start.elapsed() > timeout {
            bail!("Timed out waiting for operator deployment to become Available (5 min)");
        }

        std::thread::sleep(interval);
    }
}

/// Ensure the TektonConfig CR exists. If the operator was just installed,
/// the CRD may not be registered yet — retries with backoff.
pub fn ensure_tektonconfig(rt: &Runtime, client: &Client) -> anyhow::Result<()> {
    let ar = ApiResource {
        group: "operator.tekton.dev".into(),
        version: "v1alpha1".into(),
        api_version: "operator.tekton.dev/v1alpha1".into(),
        kind: "TektonConfig".into(),
        plural: "tektonconfigs".into(),
    };

    let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);

    // Check if already exists
    if rt.block_on(api.get("config")).is_ok() {
        eprintln!("  TektonConfig 'config' already exists.");
        return Ok(());
    }

    let tc: DynamicObject = serde_json::from_value(json!({
        "apiVersion": "operator.tekton.dev/v1alpha1",
        "kind": "TektonConfig",
        "metadata": {
            "name": "config"
        },
        "spec": {
            "targetNamespace": "openshift-pipelines",
            "profile": "all"
        }
    }))?;

    // Retry with backoff in case CRD isn't registered yet
    let max_retries = 6;
    let mut backoff = std::time::Duration::from_secs(5);

    for attempt in 1..=max_retries {
        match rt.block_on(api.create(&PostParams::default(), &tc)) {
            Ok(_) => {
                eprintln!("  Created TektonConfig 'config'.");
                return Ok(());
            }
            Err(e) if attempt < max_retries => {
                eprintln!(
                    "  Attempt {attempt}/{max_retries} to create TektonConfig failed: {e}. Retrying in {}s...",
                    backoff.as_secs()
                );
                std::thread::sleep(backoff);
                backoff *= 2;
            }
            Err(e) => {
                return Err(e).context("Failed to create TektonConfig after retries");
            }
        }
    }

    Ok(())
}
