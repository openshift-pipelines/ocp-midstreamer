use anyhow::{Context, Result};
use serde::Serialize;
use std::fs;
use std::path::Path;

/// Konflux SNAPSHOT format.
#[derive(Serialize)]
pub struct Snapshot {
    pub components: Vec<SnapshotComponent>,
}

#[derive(Serialize)]
pub struct SnapshotComponent {
    pub name: String,
    #[serde(rename = "containerImage")]
    pub container_image: String,
}

/// Generate a Konflux-compatible SNAPSHOT JSON file.
pub fn generate_snapshot(index_pullspec: &str, output_path: &Path) -> Result<()> {
    let snapshot = Snapshot {
        components: vec![SnapshotComponent {
            name: "fbc-index".to_string(),
            container_image: index_pullspec.to_string(),
        }],
    };

    let json = serde_json::to_string_pretty(&snapshot)
        .context("Failed to serialize SNAPSHOT")?;

    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(output_path, &json)
        .context("Failed to write SNAPSHOT file")?;

    eprintln!("  SNAPSHOT written to {}", output_path.display());
    Ok(())
}
