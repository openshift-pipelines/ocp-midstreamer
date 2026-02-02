use anyhow::bail;

use crate::config::Config;

/// Build image mappings from built image names to IMAGE_ env var keys.
///
/// For each built image, looks up the corresponding IMAGE_ env var from config.
/// Returns vec of (env_var_key, full_image_ref) pairs.
pub fn build_image_mappings(
    config: &Config,
    component: &str,
    registry: &str,
    built_images: &[String],
) -> anyhow::Result<Vec<(String, String)>> {
    let comp = config
        .components
        .get(component)
        .ok_or_else(|| anyhow::anyhow!("Component '{component}' not found in config"))?;

    let mut mappings = Vec::new();
    for image_name in built_images {
        let env_var = comp
            .images
            .get(image_name.as_str())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No IMAGE_ mapping found for built image '{image_name}' in component '{component}'"
                )
            })?;

        let full_ref = format!("{registry}/{image_name}");
        mappings.push((env_var.clone(), full_ref));
    }

    if mappings.is_empty() {
        bail!("No image mappings produced for component '{component}'");
    }

    Ok(mappings)
}

/// Display a formatted table of IMAGE_ env var mappings to stderr.
pub fn display_mapping_table(mappings: &[(String, String)]) {
    let max_key_len = mappings.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    eprintln!();
    eprintln!("  {:<width$}  IMAGE", "ENV VAR", width = max_key_len);
    eprintln!("  {:<width$}  -----", "-------", width = max_key_len);
    for (key, value) in mappings {
        eprintln!("  {:<width$}  {}", key, value, width = max_key_len);
    }
    eprintln!();
}
