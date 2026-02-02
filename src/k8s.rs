use anyhow::Context;

/// Creates a kube client using the default kubeconfig/in-cluster config.
/// Returns both the tokio Runtime (needed for subsequent async calls) and the Client.
pub fn create_kube_client() -> anyhow::Result<(tokio::runtime::Runtime, kube::Client)> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Failed to create tokio runtime")?;

    let client = rt
        .block_on(kube::Client::try_default())
        .context("Failed to connect to cluster. Are you logged in? Try: oc login")?;

    Ok((rt, client))
}
