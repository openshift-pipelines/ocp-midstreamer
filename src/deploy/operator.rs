use anyhow::{bail, Context};
use k8s_openapi::api::apps::v1::Deployment;
use kube::api::{Api, ApiResource, DynamicObject, ListParams};
use kube::Client;
use serde_json::json;
use tokio::runtime::Runtime;

/// Verify that the OpenShift Pipelines operator is installed by checking for the TektonConfig CR.
pub fn verify_operator(rt: &Runtime, client: &Client) -> anyhow::Result<DynamicObject> {
    let ar = ApiResource {
        group: "operator.tekton.dev".into(),
        version: "v1alpha1".into(),
        api_version: "operator.tekton.dev/v1alpha1".into(),
        kind: "TektonConfig".into(),
        plural: "tektonconfigs".into(),
    };

    let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);

    let result = rt.block_on(api.get("config"));

    match result {
        Ok(tc) => Ok(tc),
        Err(kube::Error::Api(ref resp)) if resp.code == 404 => {
            bail!("OpenShift Pipelines operator not found. TektonConfig CR 'config' does not exist.\nIs the operator installed?")
        }
        Err(kube::Error::Api(ref resp)) if resp.code == 403 => {
            bail!("Insufficient permissions to read TektonConfig CR.\nEnsure you have cluster-admin or appropriate RBAC.")
        }
        Err(e) => Err(e).context("Failed to verify operator installation"),
    }
}

/// Known namespaces where the operator controller deployment may live.
const OPERATOR_NAMESPACES: &[&str] = &[
    "openshift-pipelines",
    "openshift-operators",
    "tekton-pipelines",
];

/// Known deployment names for the operator controller.
const OPERATOR_DEPLOYMENT_NAMES: &[&str] = &[
    "openshift-pipelines-operator",
    "tekton-operator",
];

/// Known label selectors for the operator controller deployment.
const OPERATOR_LABEL_SELECTORS: &[&str] = &[
    "app.kubernetes.io/name=openshift-pipelines-operator",
    "app=tekton-operator",
];

/// Find the operator controller deployment by checking known namespaces, names, and label selectors.
/// Returns (namespace, deployment_name).
pub fn find_operator_deployment(
    rt: &Runtime,
    client: &Client,
) -> anyhow::Result<(String, String)> {
    // First try known deployment names directly (most reliable for OLM-managed operators)
    for ns in OPERATOR_NAMESPACES {
        let api: Api<Deployment> = Api::namespaced(client.clone(), ns);
        for name in OPERATOR_DEPLOYMENT_NAMES {
            if let Ok(dep) = rt.block_on(api.get(name)) {
                if let Some(n) = &dep.metadata.name {
                    return Ok((ns.to_string(), n.clone()));
                }
            }
        }
    }

    // Fall back to label selectors
    for ns in OPERATOR_NAMESPACES {
        let api: Api<Deployment> = Api::namespaced(client.clone(), ns);
        for selector in OPERATOR_LABEL_SELECTORS {
            let lp = kube::api::ListParams::default().labels(selector);
            let list = rt
                .block_on(api.list(&lp))
                .with_context(|| format!("Failed to list deployments in namespace {ns}"))?;
            if let Some(dep) = list.items.first() {
                if let Some(name) = &dep.metadata.name {
                    return Ok((ns.to_string(), name.clone()));
                }
            }
        }
    }

    bail!(
        "Operator controller deployment not found.\nChecked namespaces: {}\nChecked names: {}\nChecked labels: {}",
        OPERATOR_NAMESPACES.join(", "),
        OPERATOR_DEPLOYMENT_NAMES.join(", "),
        OPERATOR_LABEL_SELECTORS.join(", ")
    )
}

/// Find the ClusterServiceVersion for OpenShift Pipelines operator.
/// Returns the CSV name. OLM manages the operator deployment via the CSV,
/// so patching the CSV (not the deployment) ensures changes persist.
pub fn find_operator_csv(
    rt: &Runtime,
    client: &Client,
    namespace: &str,
) -> anyhow::Result<String> {
    let ar = ApiResource {
        group: "operators.coreos.com".into(),
        version: "v1alpha1".into(),
        api_version: "operators.coreos.com/v1alpha1".into(),
        kind: "ClusterServiceVersion".into(),
        plural: "clusterserviceversions".into(),
    };

    let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), namespace, &ar);
    let lp = ListParams::default();
    let csvs = rt
        .block_on(api.list(&lp))
        .context("Failed to list CSVs")?;

    for csv in &csvs.items {
        if let Some(name) = &csv.metadata.name {
            if name.contains("openshift-pipelines-operator") {
                return Ok(name.clone());
            }
        }
    }

    bail!("OpenShift Pipelines CSV not found in namespace {namespace}")
}

/// Patch the operator CSV with IMAGE_ env vars.
/// OLM owns the deployment, so patching the deployment directly gets reverted.
/// Patching the CSV causes OLM to propagate the env var changes to the deployment.
pub fn patch_operator_images(
    rt: &Runtime,
    client: &Client,
    namespace: &str,
    csv_name: &str,
    deployment_name: &str,
    mappings: &[(String, String)],
) -> anyhow::Result<()> {
    let ar = ApiResource {
        group: "operators.coreos.com".into(),
        version: "v1alpha1".into(),
        api_version: "operators.coreos.com/v1alpha1".into(),
        kind: "ClusterServiceVersion".into(),
        plural: "clusterserviceversions".into(),
    };

    let api: Api<DynamicObject> = Api::namespaced_with(client.clone(), namespace, &ar);

    // Read current CSV to find the deployment index and merge env vars
    let csv = rt
        .block_on(api.get(csv_name))
        .with_context(|| format!("Failed to get CSV {csv_name}"))?;

    let deployments = csv.data
        .pointer("/spec/install/spec/deployments")
        .and_then(|v| v.as_array())
        .context("CSV has no deployments in spec.install.spec.deployments")?;

    // Find the deployment index matching our target
    let dep_index = deployments
        .iter()
        .position(|d| d.get("name").and_then(|n| n.as_str()) == Some(deployment_name))
        .with_context(|| format!("Deployment {deployment_name} not found in CSV"))?;

    // Find the container and merge env vars (update matching, keep others)
    let containers = deployments[dep_index]
        .pointer("/spec/template/spec/containers")
        .and_then(|v| v.as_array())
        .context("No containers in deployment spec")?;

    let container_index = containers
        .iter()
        .position(|c| {
            c.get("name").and_then(|n| n.as_str()) == Some("openshift-pipelines-operator-lifecycle")
        })
        .context("Container 'openshift-pipelines-operator-lifecycle' not found in CSV")?;

    let existing_envs = containers[container_index]
        .get("env")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // Build new env list: replace matching keys, keep the rest
    let mut new_envs: Vec<serde_json::Value> = existing_envs
        .into_iter()
        .map(|mut env| {
            if let Some(name) = env.get("name").and_then(|n| n.as_str()) {
                if let Some((_, new_val)) = mappings.iter().find(|(k, _)| k == name) {
                    env.as_object_mut().map(|obj| {
                        obj.insert("value".into(), json!(new_val));
                        obj.remove("valueFrom");
                    });
                }
            }
            env
        })
        .collect();

    // Add any new keys not already present
    for (key, value) in mappings {
        if !new_envs.iter().any(|e| e.get("name").and_then(|n| n.as_str()) == Some(key)) {
            new_envs.push(json!({"name": key, "value": value}));
        }
    }

    // Modify the CSV object in-place and replace it
    let mut csv_data = csv.data.clone();
    let env_pointer = format!(
        "/spec/install/spec/deployments/{}/spec/template/spec/containers/{}/env",
        dep_index, container_index
    );
    if let Some(target) = csv_data.pointer_mut(&env_pointer) {
        *target = json!(new_envs);
    } else {
        bail!("Could not find env path in CSV: {}", env_pointer);
    }

    let mut patched_csv = csv.clone();
    patched_csv.data = csv_data;

    let pp = kube::api::PostParams::default();
    rt.block_on(api.replace(csv_name, &pp, &patched_csv))
        .with_context(|| format!("Failed to update CSV {csv_name}"))?;

    Ok(())
}

/// Delete TektonInstallerSets matching a component to force the operator to re-reconcile.
/// The operator uses IMAGE_ env vars when creating InstallerSets, so deleting them
/// causes recreation with the new (upstream) images.
pub fn delete_installer_sets(
    rt: &Runtime,
    client: &Client,
    component: &str,
    prefix_override: Option<&str>,
) -> anyhow::Result<u32> {
    let ar = ApiResource {
        group: "operator.tekton.dev".into(),
        version: "v1alpha1".into(),
        api_version: "operator.tekton.dev/v1alpha1".into(),
        kind: "TektonInstallerSet".into(),
        plural: "tektoninstallersets".into(),
    };

    let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);
    let lp = ListParams::default();
    let sets = rt
        .block_on(api.list(&lp))
        .context("Failed to list TektonInstallerSets")?;

    // Match installer sets by component prefix (e.g. "pipeline-main-deployment-*")
    // Use prefix_override if provided (e.g. "manualapprovalgate" for manual-approval-gate)
    let prefix = prefix_override.unwrap_or(component);
    let prefixes: Vec<String> = vec![
        format!("{}-main-deployment-", prefix),
        format!("{}-main-static-", prefix),
        format!("{}-post-", prefix),
        format!("{}-pre-", prefix),
    ];

    let mut deleted = 0u32;
    for set in &sets.items {
        if let Some(name) = &set.metadata.name {
            if prefixes.iter().any(|p| name.starts_with(p)) {
                let dp = kube::api::DeleteParams::default();
                match rt.block_on(api.delete(name, &dp)) {
                    Ok(_) => {
                        eprintln!("  Deleted InstallerSet: {}", name);
                        deleted += 1;
                    }
                    Err(e) => {
                        eprintln!("  WARNING: Failed to delete InstallerSet {}: {}", name, e);
                    }
                }
            }
        }
    }

    Ok(deleted)
}

/// Ensure all authenticated users can pull images from the upstream namespace.
/// Tekton uses entrypoint/nop/workingdirinit as init containers in arbitrary user
/// namespaces, so we need cluster-wide pull access — not just specific namespaces.
pub fn ensure_image_pull_rbac(
    rt: &Runtime,
    client: &Client,
    image_namespace: &str,
) -> anyhow::Result<()> {
    use k8s_openapi::api::rbac::v1::RoleBinding;

    let api: Api<RoleBinding> = Api::namespaced(client.clone(), image_namespace);

    let binding_name = "image-puller-all-authenticated";

    // Check if it already exists
    if rt.block_on(api.get(binding_name)).is_ok() {
        return Ok(());
    }

    let rb: RoleBinding = serde_json::from_value(json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "RoleBinding",
        "metadata": {
            "name": binding_name,
            "namespace": image_namespace,
        },
        "roleRef": {
            "apiGroup": "rbac.authorization.k8s.io",
            "kind": "ClusterRole",
            "name": "system:image-puller",
        },
        "subjects": [{
            "apiGroup": "rbac.authorization.k8s.io",
            "kind": "Group",
            "name": "system:authenticated",
        }]
    }))?;

    rt.block_on(api.create(&kube::api::PostParams::default(), &rb))
        .with_context(|| {
            format!(
                "Failed to create image-puller RoleBinding in {}",
                image_namespace
            )
        })?;
    eprintln!("  Granted image-puller to all authenticated users in {}", image_namespace);

    Ok(())
}
