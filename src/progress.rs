use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

pub fn stage_spinner(message: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
            .template("{spinner} {msg}")
            .expect("invalid spinner template"),
    );
    pb.set_message(message.to_string());
    pb.enable_steady_tick(std::time::Duration::from_millis(80));
    pb
}

pub fn finish_spinner(pb: &ProgressBar, success: bool) {
    if success {
        pb.finish_with_message(format!("✓ {}", pb.message()));
    } else {
        pb.finish_with_message(format!("✗ {}", pb.message()));
    }
}

/// Create a MultiProgress instance for parallel component builds.
pub fn multi_progress() -> MultiProgress {
    MultiProgress::new()
}

/// Add a spinner to a MultiProgress for a named component.
pub fn component_spinner(mp: &MultiProgress, component: &str) -> ProgressBar {
    let pb = mp.add(ProgressBar::new_spinner());
    pb.set_style(
        ProgressStyle::default_spinner()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
            .template("{spinner} [{elapsed}] {msg}")
            .expect("invalid spinner template"),
    );
    pb.set_message(format!("{component}: waiting..."));
    pb.enable_steady_tick(std::time::Duration::from_millis(80));
    pb
}
