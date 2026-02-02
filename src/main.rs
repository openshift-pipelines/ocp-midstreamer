mod build;
mod check;
mod cli;
mod component;
mod config;
mod deploy;
mod dryrun;
mod exec;
mod incluster;
mod k8s;
mod profile;
mod progress;
mod publish;
mod registry;
mod results;
mod setup;
mod test;
mod types;

use clap::Parser;
use cli::{Cli, Commands};

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Check { fix } => {
            match check::run_check(cli.verbose) {
                Ok(true) => {
                    if fix {
                        eprintln!("\nAll checks passed, nothing to fix.");
                    }
                    std::process::exit(0);
                }
                Ok(false) => {
                    if fix {
                        eprintln!("\nRunning auto-setup to fix issues...");
                        if let Err(e) = setup::run_auto_setup() {
                            eprintln!("Auto-setup error: {e:#}");
                            std::process::exit(2);
                        }
                        std::process::exit(0);
                    }
                    std::process::exit(1);
                }
                Err(e) => {
                    eprintln!("Error: {e:#}");
                    std::process::exit(2);
                }
            }
        }
        Commands::Build { component } => {
            if !cli.no_auto_setup {
                let result = tokio::task::spawn_blocking(|| {
                    setup::run_auto_setup()
                }).await;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => eprintln!("WARNING: Auto-setup had errors: {e:#}"),
                    Err(e) => eprintln!("WARNING: Auto-setup panicked: {e}"),
                }
            }
            match run_build(&component) {
                Ok(_) => std::process::exit(0),
                Err(e) => {
                    eprintln!("Error: {e:#}");
                    std::process::exit(2);
                }
            }
        }
        Commands::Deploy {
            component,
            registry,
        } => {
            if !cli.no_auto_setup {
                let result = tokio::task::spawn_blocking(|| {
                    setup::run_auto_setup()
                }).await;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => eprintln!("WARNING: Auto-setup had errors: {e:#}"),
                    Err(e) => eprintln!("WARNING: Auto-setup panicked: {e}"),
                }
            }
            // Placeholder: in production, built_images comes from the build phase output.
            // For now, derive image names from the TOML config for the given component.
            let built_images = match load_image_names_from_config(&component) {
                Ok(names) => names,
                Err(e) => {
                    eprintln!("Error: {e:#}");
                    std::process::exit(2);
                }
            };
            eprintln!("Note: using image names from config (placeholder until build phase integration)");
            let verbose = cli.verbose;
            let result = tokio::task::spawn_blocking(move || {
                deploy::run_deploy(&component, &registry, &built_images, verbose)
            }).await;
            match result {
                Ok(Ok(_)) => std::process::exit(0),
                Ok(Err(e)) => {
                    eprintln!("Error: {e:#}");
                    std::process::exit(2);
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(2);
                }
            }
        }
        Commands::Test {
            tags,
            release_tests_ref,
            output_dir,
            profile,
        } => {
            match test::run_tests(&tags, &release_tests_ref, std::path::Path::new(&output_dir), cli.verbose, profile).await {
                Ok(true) => std::process::exit(0),
                Ok(false) => std::process::exit(1),
                Err(e) => {
                    eprintln!("Error: {e:#}");
                    std::process::exit(2);
                }
            }
        }
        Commands::Run {
            components,
            dry_run,
            json,
            tags,
            release_tests_ref,
            output_dir,
            registry,
            skip_build,
            profile,
        } => {
            let specs = match components {
                Some(ref s) => match component::parse_component_specs(s) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("Error: {e}");
                        std::process::exit(2);
                    }
                },
                None => component::default_specs(),
            };

            if !cli.no_auto_setup && !skip_build && !incluster::is_incluster() {
                let result = tokio::task::spawn_blocking(|| {
                    setup::run_auto_setup()
                }).await;
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => eprintln!("WARNING: Auto-setup had errors: {e:#}"),
                    Err(e) => eprintln!("WARNING: Auto-setup panicked: {e}"),
                }
            }

            if skip_build {
                // In-cluster mode: skip clone/build, go straight to deploy+test
                let exit_code = run_deploy_and_test(&specs, &tags, &release_tests_ref, &output_dir, registry.as_deref(), cli.verbose, profile, cli.no_auto_setup).await;
                std::process::exit(exit_code);
            }

            if incluster::is_incluster() {
                // Already in-cluster: run deploy+test directly (don't re-wrap)
                let exit_code = run_deploy_and_test(&specs, &tags, &release_tests_ref, &output_dir, registry.as_deref(), cli.verbose, profile, cli.no_auto_setup).await;
                std::process::exit(exit_code);
            }

            // Normal mode: build locally, then create in-cluster Job for deploy+test
            let exit_code = run_multi(specs, dry_run, json, &tags, &release_tests_ref, &output_dir, registry.as_deref(), cli.verbose).await;
            std::process::exit(exit_code);
        }
        Commands::Results { output_dir } => {
            let output_path = std::path::Path::new(&output_dir);
            let results_dir = output_path.join("results");
            if let Err(e) = std::fs::create_dir_all(&results_dir) {
                eprintln!("Error creating results directory: {e:#}");
                std::process::exit(2);
            }

            // Try JUnit XML first, then fall back to Gauge stdout
            let junit_path = results_dir.join("junit.xml");
            let stdout_path = output_path.join("logs/test-stdout.log");

            let parse_result = if junit_path.exists() {
                results::parse_junit_xml(&junit_path)
            } else if stdout_path.exists() {
                results::parse_gauge_stdout(&stdout_path)
            } else {
                eprintln!("No test results found in {}", output_dir);
                eprintln!("Expected: {}/results/junit.xml or {}/logs/test-stdout.log", output_dir, output_dir);
                std::process::exit(2);
            };

            match parse_result {
                Ok(result) => {
                    let categorized = results::categorize_results(&result);
                    results::print_categorized_results(&categorized);

                    let json_path = results_dir.join("results.json");
                    if let Err(e) = results::write_categorized_json(&categorized, &json_path) {
                        eprintln!("Error writing JSON: {e:#}");
                        std::process::exit(2);
                    }
                    println!("Results written to {}", json_path.display());
                    std::process::exit(0);
                }
                Err(e) => {
                    eprintln!("Error parsing test results: {e:#}");
                    std::process::exit(2);
                }
            }
        }
        Commands::Status => {
            let client = match kube::Client::try_default().await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error connecting to cluster: {e:#}");
                    std::process::exit(2);
                }
            };
            let namespace = "openshift-pipelines";
            if let Err(e) = incluster::show_status(&client, namespace).await {
                eprintln!("Error: {e:#}");
                std::process::exit(2);
            }
        }
        Commands::Publish { output_dir, remote, label } => {
            match publish::publish(&output_dir, remote.as_deref(), label.as_deref()) {
                Ok(()) => std::process::exit(0),
                Err(e) => {
                    eprintln!("Error: {e:#}");
                    std::process::exit(2);
                }
            }
        }
        Commands::Logs { job } => {
            let client = match kube::Client::try_default().await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("Error connecting to cluster: {e:#}");
                    std::process::exit(2);
                }
            };
            let namespace = "openshift-pipelines";
            if let Err(e) = incluster::stream_job_logs(&client, namespace, job.as_deref()).await {
                eprintln!("Error: {e:#}");
                std::process::exit(2);
            }
        }
    }
}

/// Deploy and test only (used in-cluster where builds already happened locally).
async fn run_deploy_and_test(
    specs: &[component::ComponentSpec],
    tags: &str,
    release_tests_ref: &str,
    output_dir: &str,
    registry_override: Option<&str>,
    verbose: bool,
    profile: bool,
    no_auto_setup: bool,
) -> i32 {
    if !no_auto_setup {
        let result = tokio::task::spawn_blocking(|| {
            setup::run_auto_setup()
        }).await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => eprintln!("WARNING: Auto-setup had errors: {e:#}"),
            Err(e) => eprintln!("WARNING: Auto-setup panicked: {e}"),
        }
    }

    let _cfg = match config::load_config(&config::default_config_path()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error loading config: {e:#}");
            return 2;
        }
    };

    let registry_route = match registry_override {
        Some(r) => r.to_string(),
        None => match registry::get_registry_route() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error: {e:#}");
                return 2;
            }
        },
    };

    // Deploy phase
    eprintln!("\n=== Deploying (in-cluster) ===");
    for spec in specs {
        let image_names = match load_image_names_from_config(&spec.name) {
            Ok(names) => names,
            Err(e) => {
                eprintln!("WARNING: Could not load images for {}: {e:#}", spec.name);
                continue;
            }
        };
        let comp_name = spec.name.clone();
        let registry_route = registry_route.clone();
        let result = tokio::task::spawn_blocking(move || {
            deploy::run_deploy(&comp_name, &registry_route, &image_names, verbose)
        })
        .await;
        match result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => eprintln!("WARNING: Deploy failed for {}: {e:#}", spec.name),
            Err(e) => eprintln!("WARNING: Deploy panicked for {}: {e}", spec.name),
        }
    }

    // Test phase
    eprintln!("\n=== Running tests (in-cluster) ===");
    match test::run_tests(tags, release_tests_ref, std::path::Path::new(output_dir), verbose, profile).await {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(e) => {
            eprintln!("Error running tests: {e:#}");
            1
        }
    }
}

/// Load image names from config for a component (placeholder for build phase output).
fn load_image_names_from_config(component: &str) -> anyhow::Result<Vec<String>> {
    let cfg = config::load_config(&config::default_config_path())?;
    let comp = cfg
        .components
        .get(component)
        .ok_or_else(|| anyhow::anyhow!("Component '{component}' not in config"))?;
    Ok(comp.images.keys().cloned().collect())
}

fn run_build(component: &str) -> anyhow::Result<()> {
    // Stage 1: Registry setup
    let pb = progress::stage_spinner("Registry setup");
    let route = registry::get_registry_route()?;
    registry::ensure_namespace(registry::DEFAULT_NAMESPACE)?;
    registry::registry_login(&route)?;
    let registry_target = format!("{}/{}", route, registry::DEFAULT_NAMESPACE);
    progress::finish_spinner(&pb, true);

    // Stage 2: Clone upstream source
    let pb = progress::stage_spinner("Clone upstream source");
    let temp_dir = tempfile::tempdir()?;
    let repo_url = format!("https://github.com/tektoncd/{}.git", component);
    build::clone_repo(&repo_url, temp_dir.path())?;
    progress::finish_spinner(&pb, true);

    // Stage 3: Build images with ko
    let cfg = config::load_config(&config::default_config_path())?;
    let comp_cfg = cfg
        .components
        .get(component)
        .ok_or_else(|| anyhow::anyhow!("Component '{}' not in config", component))?;

    let pb = progress::stage_spinner("Build images with ko");
    let image_names = build::ko_build(temp_dir.path(), &registry_target, &comp_cfg.import_paths)?;
    progress::finish_spinner(&pb, true);

    println!("\nBuilt {} images for {}:", image_names.len(), component);
    for name in &image_names {
        println!("  - {}", name);
    }

    Ok(())
}

/// Multi-component orchestration: build all in parallel, then create in-cluster Job for deploy+test.
/// Returns exit code: 0=success, 2=error.
async fn run_multi(
    specs: Vec<component::ComponentSpec>,
    dry_run: bool,
    json_output: bool,
    tags: &str,
    release_tests_ref: &str,
    output_dir: &str,
    registry_override: Option<&str>,
    _verbose: bool,
) -> i32 {
    let cfg = match config::load_config(&config::default_config_path()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error loading config: {e:#}");
            return 2;
        }
    };

    // Dry-run: just print the plan
    if dry_run {
        return print_dry_run_plan(&specs, &cfg, json_output);
    }

    // Registry setup
    let registry_route = match registry_override {
        Some(r) => r.to_string(),
        None => match registry::get_registry_route() {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Error: {e:#}");
                return 2;
            }
        },
    };

    if let Err(e) = registry::ensure_namespace(registry::DEFAULT_NAMESPACE) {
        eprintln!("Error ensuring namespace: {e:#}");
        return 2;
    }
    if let Err(e) = registry::registry_login(&registry_route) {
        eprintln!("Error logging into registry: {e:#}");
        return 2;
    }

    let registry_target = format!("{}/{}", registry_route, registry::DEFAULT_NAMESPACE);

    // Build phase: build all components in parallel
    eprintln!("\n=== Building components in parallel ===");
    let results = build::build_components_parallel(&specs, &cfg.components, &registry_target).await;

    let mut all_images: Vec<(String, Vec<String>)> = Vec::new();
    let mut build_failed = false;

    for (name, result) in results {
        match result {
            Ok(images) => {
                eprintln!("  {} built {} images", name, images.len());
                all_images.push((name, images));
            }
            Err(e) => {
                eprintln!("  {} FAILED: {e:#}", name);
                build_failed = true;
            }
        }
    }

    if build_failed {
        return 2;
    }

    // Deploy+test phase: create in-cluster Job instead of running locally
    eprintln!("\n=== Creating in-cluster Job for deploy+test ===");
    let spec_str = specs.iter().map(|s| {
        match &s.git_ref {
            Some(r) => format!("{}:{}", s.name, r),
            None => s.name.clone(),
        }
    }).collect::<Vec<_>>().join(",");
    let mut cli_args = vec![
        "run".to_string(),
        "--components".to_string(), spec_str,
        "--tags".to_string(), tags.to_string(),
        "--release-tests-ref".to_string(), release_tests_ref.to_string(),
        "--output-dir".to_string(), output_dir.to_string(),
    ];
    if let Some(reg) = registry_override {
        cli_args.push("--registry".to_string());
        cli_args.push(reg.to_string());
    }

    let registry_route_clone = registry_route.clone();
    let result = tokio::task::spawn_blocking(move || {
        incluster::run_incluster(&registry_route_clone, "openshift-pipelines", &cli_args)
    }).await;
    match result {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => { eprintln!("Error creating in-cluster Job: {e:#}"); 2 }
        Err(e) => { eprintln!("Error: in-cluster task panicked: {e}"); 2 }
    }
}

/// Print the dry-run execution plan using the dryrun module.
fn print_dry_run_plan(
    specs: &[component::ComponentSpec],
    cfg: &config::Config,
    json_output: bool,
) -> i32 {
    let resolved = dryrun::resolve_components(specs, &cfg.components);
    if json_output {
        dryrun::print_json(&resolved);
    } else {
        dryrun::print_table(&resolved);
    }
    0
}
