use anyhow::{Context, Result};
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{Pod, ServiceAccount};
use k8s_openapi::api::rbac::v1::ClusterRoleBinding;
use kube::api::{Api, ListParams, LogParams, PostParams};
use futures::{AsyncBufReadExt, TryStreamExt};

/// Returns the CLI version tag for image caching.
pub fn cli_image_tag() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Returns the full image reference for the CLI container.
pub fn cli_image_ref(registry: &str) -> String {
    format!(
        "{}/openshift-pipelines/streamstress-cli:{}",
        registry,
        cli_image_tag()
    )
}

/// Check if the cached CLI image already exists in the registry.
pub fn image_exists(registry: &str) -> Result<bool> {
    let image_ref = cli_image_ref(registry);
    let result = crate::exec::run_cmd_unchecked("oc", &["image", "info", &image_ref]);
    match result {
        Ok(r) => Ok(r.exit_code == 0),
        Err(_) => Ok(false),
    }
}

/// Build and push the CLI container image, using version-based caching.
pub fn build_and_push_cli_image(registry: &str) -> Result<()> {
    let image_ref = cli_image_ref(registry);

    if image_exists(registry).unwrap_or(false) {
        eprintln!("Using cached CLI image {}", image_ref);
        return Ok(());
    }

    eprintln!("Building CLI image {}...", image_ref);
    crate::exec::run_cmd("podman", &["build", "-f", "Dockerfile.cli", "-t", &image_ref, "."])
        .context("Failed to build CLI container image")?;

    eprintln!("Pushing CLI image {}...", image_ref);
    crate::exec::run_cmd("podman", &["push", &image_ref])
        .context("Failed to push CLI container image")?;

    eprintln!("CLI image pushed successfully.");
    Ok(())
}

/// Detect if already running inside a Kubernetes pod.
pub fn is_incluster() -> bool {
    std::path::Path::new("/var/run/secrets/kubernetes.io/serviceaccount/token").exists()
}

/// Ensure the ServiceAccount and ClusterRoleBinding exist for in-cluster execution.
pub async fn ensure_service_account(client: &kube::Client, namespace: &str) -> Result<()> {
    let sa_api: Api<ServiceAccount> = Api::namespaced(client.clone(), namespace);
    let sa = serde_json::from_value(serde_json::json!({
        "apiVersion": "v1",
        "kind": "ServiceAccount",
        "metadata": {
            "name": "streamstress-sa",
            "namespace": namespace
        }
    }))?;

    match sa_api.create(&PostParams::default(), &sa).await {
        Ok(_) => eprintln!("Created ServiceAccount streamstress-sa"),
        Err(kube::Error::Api(ae)) if ae.code == 409 => {
            // Already exists
        }
        Err(e) => return Err(e).context("Failed to create ServiceAccount"),
    }

    let crb_api: Api<ClusterRoleBinding> = Api::all(client.clone());
    let crb = serde_json::from_value(serde_json::json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "ClusterRoleBinding",
        "metadata": {
            "name": "streamstress-crb"
        },
        "roleRef": {
            "apiGroup": "rbac.authorization.k8s.io",
            "kind": "ClusterRole",
            "name": "cluster-admin"
        },
        "subjects": [{
            "kind": "ServiceAccount",
            "name": "streamstress-sa",
            "namespace": namespace
        }]
    }))?;

    match crb_api.create(&PostParams::default(), &crb).await {
        Ok(_) => eprintln!("Created ClusterRoleBinding streamstress-crb"),
        Err(kube::Error::Api(ae)) if ae.code == 409 => {
            // Already exists
        }
        Err(e) => return Err(e).context("Failed to create ClusterRoleBinding"),
    }

    Ok(())
}

/// Publish configuration passed to the Job for direct gh-pages push.
#[derive(Debug, Clone, Default)]
pub struct PublishEnv {
    pub github_token: Option<String>,
    pub github_repository: Option<String>,
    pub label: Option<String>,
    pub output_dir: Option<String>,
}

impl PublishEnv {
    /// Load publish config from current environment (for passing to Job).
    pub fn from_env() -> Self {
        Self {
            github_token: std::env::var("GITHUB_TOKEN").ok(),
            github_repository: std::env::var("GITHUB_REPOSITORY").ok(),
            label: std::env::var("RUN_LABEL").ok(),
            output_dir: std::env::var("OUTPUT_DIR").ok(),
        }
    }

    /// Check if publish is configured.
    pub fn is_configured(&self) -> bool {
        self.github_token.is_some() && self.github_repository.is_some()
    }
}

/// Create a detached Kubernetes Job for in-cluster execution. Returns the Job name.
pub async fn create_job(
    client: &kube::Client,
    namespace: &str,
    image_ref: &str,
    cli_args: &[String],
    publish_env: &PublishEnv,
) -> Result<String> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let job_name = format!("streamstress-{}", timestamp);

    let args_json: Vec<serde_json::Value> = cli_args.iter().map(|a| serde_json::json!(a)).collect();

    // Build env vars for direct gh-pages publish (if configured)
    let mut env_vars: Vec<serde_json::Value> = vec![
        serde_json::json!({"name": "JOB_NAME", "value": &job_name}),
    ];

    if let Some(ref token) = publish_env.github_token {
        env_vars.push(serde_json::json!({"name": "GITHUB_TOKEN", "value": token}));
    }
    if let Some(ref repo) = publish_env.github_repository {
        env_vars.push(serde_json::json!({"name": "GITHUB_REPOSITORY", "value": repo}));
    }
    if let Some(ref label) = publish_env.label {
        env_vars.push(serde_json::json!({"name": "RUN_LABEL", "value": label}));
    }
    if let Some(ref output_dir) = publish_env.output_dir {
        env_vars.push(serde_json::json!({"name": "OUTPUT_DIR", "value": output_dir}));
    }

    let job: Job = serde_json::from_value(serde_json::json!({
        "apiVersion": "batch/v1",
        "kind": "Job",
        "metadata": {
            "name": &job_name,
            "namespace": namespace,
            "labels": {
                "app": "streamstress"
            }
        },
        "spec": {
            "backoffLimit": 0,
            "activeDeadlineSeconds": 10800,
            "template": {
                "metadata": {
                    "labels": {
                        "app": "streamstress",
                        "job-name": &job_name
                    }
                },
                "spec": {
                    "serviceAccountName": "streamstress-sa",
                    "restartPolicy": "Never",
                    "containers": [{
                        "name": "streamstress",
                        "image": image_ref,
                        "imagePullPolicy": "Always",
                        "args": args_json,
                        "env": env_vars
                    }]
                }
            }
        }
    }))?;

    let jobs_api: Api<Job> = Api::namespaced(client.clone(), namespace);
    jobs_api
        .create(&PostParams::default(), &job)
        .await
        .context("Failed to create Job")?;

    Ok(job_name)
}

/// Main entry point for in-cluster execution. Builds image, creates Job, returns immediately.
/// Internal service address for the OCP image registry.
/// Pods pull from this address (no auth needed with proper RBAC).
const INTERNAL_REGISTRY: &str = "image-registry.openshift-image-registry.svc:5000";

pub fn run_incluster(registry: &str, namespace: &str, cli_args: &[String]) -> Result<()> {
    // Push to external route, but Job pulls via internal service address
    build_and_push_cli_image(registry)?;

    let image_ref = cli_image_ref(INTERNAL_REGISTRY);

    // Append --skip-build so the in-cluster copy skips clone/build
    let mut job_args = cli_args.to_vec();
    if !job_args.contains(&"--skip-build".to_string()) {
        job_args.push("--skip-build".to_string());
    }

    // Load publish env from current environment (CI passes these)
    let publish_env = PublishEnv::from_env();
    if publish_env.is_configured() {
        eprintln!("Auto-publish configured: results will be pushed to gh-pages when Job completes");
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Failed to create tokio runtime")?;

    let client = rt
        .block_on(kube::Client::try_default())
        .context("Failed to connect to cluster")?;

    rt.block_on(ensure_service_account(&client, namespace))?;
    let job_name = rt.block_on(create_job(&client, namespace, &image_ref, &job_args, &publish_env))?;

    eprintln!("Job {} created in namespace {}", job_name, namespace);
    eprintln!("  View status:  streamstress status");
    eprintln!("  Stream logs:  streamstress logs");

    Ok(())
}

/// Show status of streamstress Jobs in the namespace.
pub async fn show_status(client: &kube::Client, namespace: &str) -> Result<()> {
    let jobs_api: Api<Job> = Api::namespaced(client.clone(), namespace);
    let lp = ListParams::default().labels("app=streamstress");
    let job_list = jobs_api.list(&lp).await.context("Failed to list Jobs")?;

    if job_list.items.is_empty() {
        println!("No streamstress Jobs found in namespace {}", namespace);
        return Ok(());
    }

    println!("{:<40} {:<12} {:<12} {:<12}", "NAME", "STATUS", "AGE", "POD PHASE");
    println!("{}", "-".repeat(76));

    let pods_api: Api<Pod> = Api::namespaced(client.clone(), namespace);

    for job in &job_list.items {
        let name = job.metadata.name.as_deref().unwrap_or("unknown");

        let status = if let Some(ref s) = job.status {
            if s.succeeded.unwrap_or(0) > 0 {
                "Succeeded"
            } else if s.failed.unwrap_or(0) > 0 {
                "Failed"
            } else if s.active.unwrap_or(0) > 0 {
                "Active"
            } else {
                "Pending"
            }
        } else {
            "Unknown"
        };

        let age = if let Some(ref ct) = job.metadata.creation_timestamp {
            let created = ct.0.as_second();
            let now = chrono_now_secs();
            format_age(now - created)
        } else {
            "N/A".to_string()
        };

        // Look up pod for this job
        let pod_lp = ListParams::default().labels(&format!("job-name={}", name));
        let pod_phase = match pods_api.list(&pod_lp).await {
            Ok(pods) => {
                if let Some(pod) = pods.items.first() {
                    pod.status
                        .as_ref()
                        .and_then(|s| s.phase.clone())
                        .unwrap_or_else(|| "Unknown".to_string())
                } else {
                    "NoPod".to_string()
                }
            }
            Err(_) => "Error".to_string(),
        };

        println!("{:<40} {:<12} {:<12} {:<12}", name, status, age, pod_phase);
    }

    Ok(())
}

/// Stream logs from the most recent (or specified) streamstress Job pod.
pub async fn stream_job_logs(
    client: &kube::Client,
    namespace: &str,
    job_name: Option<&str>,
) -> Result<()> {
    let jobs_api: Api<Job> = Api::namespaced(client.clone(), namespace);

    let target_job_name = if let Some(name) = job_name {
        name.to_string()
    } else {
        // Find the most recent Job
        let lp = ListParams::default().labels("app=streamstress");
        let job_list = jobs_api.list(&lp).await.context("Failed to list Jobs")?;
        let most_recent = job_list
            .items
            .iter()
            .max_by_key(|j| {
                j.metadata
                    .creation_timestamp
                    .as_ref()
                    .map(|t| t.0.as_second())
                    .unwrap_or(0)
            })
            .ok_or_else(|| anyhow::anyhow!("No streamstress Jobs found"))?;
        most_recent
            .metadata
            .name
            .clone()
            .unwrap_or_else(|| "unknown".to_string())
    };

    eprintln!("Streaming logs for Job {}...", target_job_name);

    let pods_api: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let pod_lp = ListParams::default().labels(&format!("job-name={}", target_job_name));

    // Wait for pod to appear (up to 60s)
    let mut pod_name = None;
    for _ in 0..30 {
        let pods = pods_api.list(&pod_lp).await?;
        if let Some(pod) = pods.items.first() {
            pod_name = pod.metadata.name.clone();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }

    let pod_name = pod_name.ok_or_else(|| anyhow::anyhow!("No pod found for Job {}", target_job_name))?;

    // Check pod phase to decide follow mode
    let pod = pods_api.get(&pod_name).await?;
    let phase = pod
        .status
        .as_ref()
        .and_then(|s| s.phase.as_deref())
        .unwrap_or("Unknown");

    let follow = matches!(phase, "Running" | "Pending");

    let log_params = LogParams {
        follow,
        ..Default::default()
    };

    let log_stream = pods_api.log_stream(&pod_name, &log_params).await?;
    let mut lines = log_stream.lines();
    while let Some(line) = lines.try_next().await? {
        println!("{}", line);
    }

    Ok(())
}

/// Get current time as Unix seconds (no chrono dependency needed).
fn chrono_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Format a duration in seconds to human-readable age.
fn format_age(seconds: i64) -> String {
    if seconds < 60 {
        format!("{}s", seconds)
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86400)
    }
}
