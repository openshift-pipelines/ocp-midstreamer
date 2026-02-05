use clap::{Parser, Subcommand};


#[derive(Parser, Debug)]
#[command(name = "ocp-midstreamer", about = "OpenShift Pipelines midstream management CLI")]
pub struct Cli {
    /// Enable verbose output
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Disable automatic cluster setup (registry route, operator install)
    #[arg(long, global = true)]
    pub no_auto_setup: bool,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Check tool prerequisites (oc, ko, git, go)
    Check {
        /// Auto-fix issues that are marked [auto-fixable] (registry route, operator install)
        #[arg(long)]
        fix: bool,
    },

    /// Build Tekton component images and push to OCP internal registry
    Build {
        /// Tekton component to build (default: pipeline)
        #[arg(long, default_value = "pipeline")]
        component: String,

        /// External registry to push images to (e.g. quay.io/ocp-midstreamer).
        /// When provided, images are pushed to this registry after ko build.
        /// When omitted, images stay in the OCP internal registry.
        #[arg(long)]
        registry: Option<String>,

        /// Build component as it existed on this date (YYYY-MM-DD).
        /// Resolves to the last commit before end-of-day UTC.
        #[arg(long, value_parser = crate::component::validate_date_format)]
        as_of: Option<String>,
    },

    /// Deploy upstream-built images to the OpenShift Pipelines operator
    Deploy {
        /// Tekton component to deploy (default: pipeline)
        #[arg(long, default_value = "pipeline")]
        component: String,

        /// OCP internal registry URL (e.g. default-route-openshift-image-registry.apps.example.com/tekton-ci)
        #[arg(long)]
        registry: String,
    },

    /// Run Gauge e2e tests from the release-tests repository
    Test {
        /// Gauge tags to filter tests (default: "e2e")
        #[arg(long, default_value = "e2e")]
        tags: String,

        /// Git ref for release-tests repo (branch, tag, or commit)
        #[arg(long, default_value = "master")]
        release_tests_ref: String,

        /// Output directory for logs and results
        #[arg(long, default_value = "./test-output")]
        output_dir: String,

        /// Collect per-spec resource usage metrics during test execution
        #[arg(long)]
        profile: bool,
    },

    /// Build, deploy, and test multiple Tekton components in one command
    Run {
        /// Components to process (e.g. "pipeline,triggers" or "pipeline:pr/123,triggers:v0.28.0")
        #[arg(long)]
        components: Option<String>,

        /// Build/test components as they existed on this date (YYYY-MM-DD).
        /// Components with explicit refs (e.g. pipeline:v0.50.0) ignore this.
        /// Resolves to the last commit before end-of-day UTC.
        #[arg(long, value_parser = crate::component::validate_date_format, conflicts_with = "date_range")]
        as_of: Option<String>,

        /// Run tests for each day in a date range (YYYY-MM-DD:YYYY-MM-DD).
        /// Executes sequentially: build-deploy-test for each date before moving to next.
        /// Results stored in output-dir/DATE/ subdirectories.
        /// Mutually exclusive with --as-of.
        #[arg(long, value_parser = crate::batch::parse_date_range, conflicts_with = "as_of")]
        date_range: Option<crate::batch::DateRange>,

        /// Print the execution plan without building, deploying, or testing
        #[arg(long)]
        dry_run: bool,

        /// Output dry-run plan as JSON (requires --dry-run)
        #[arg(long, requires = "dry_run")]
        json: bool,

        /// Gauge tags to filter tests (default: "e2e")
        #[arg(long, default_value = "e2e")]
        tags: String,

        /// Git ref for release-tests repo (branch, tag, or commit)
        #[arg(long, default_value = "master")]
        release_tests_ref: String,

        /// Output directory for logs and results
        #[arg(long, default_value = "./test-output")]
        output_dir: String,

        /// OCP internal registry URL (auto-detected if not provided)
        #[arg(long)]
        registry: Option<String>,

        /// Skip clone/build phase (used by in-cluster Jobs)
        #[arg(long, hide = true)]
        skip_build: bool,

        /// Collect per-spec resource usage metrics during test execution
        #[arg(long)]
        profile: bool,

        /// Run performance tests from openshift-pipelines/performance repo.
        /// Can be combined with functional tests or run standalone.
        #[arg(long)]
        perf: bool,

        /// Performance test scenario to run (default: math).
        /// Options: math, build, signing-ongoing, cluster-resolver
        #[arg(long, default_value = "math", requires = "perf")]
        perf_scenario: String,

        /// Git ref for performance repo (branch/tag/commit).
        /// Defaults to main.
        #[arg(long)]
        perf_ref: Option<String>,
    },

    /// Re-analyze test results from a previous run
    Results {
        /// Directory containing test output (logs/ and results/ subdirs)
        #[arg(long, default_value = "./test-output")]
        output_dir: String,
    },

    /// Show status of running/completed midstreamer Jobs
    Status,

    /// Stream logs from a midstreamer Job pod
    Logs {
        /// Job name to stream logs from (default: most recent)
        #[arg(long)]
        job: Option<String>,
    },

    /// Build Konflux-compatible SNAPSHOT and optionally trigger standalone release-test-pipeline
    Konflux {
        /// External registry for pushing images (e.g. quay.io/ocp-midstreamer)
        #[arg(long)]
        registry: String,

        /// Operator repo branch to clone (e.g. main, release-v1.16)
        #[arg(long, default_value = "main")]
        operator_branch: String,

        /// Output directory for SNAPSHOT and artifacts
        #[arg(long, default_value = "./konflux-output")]
        output_dir: String,

        /// Components to include (e.g. "pipeline,triggers")
        #[arg(long, default_value = "pipeline")]
        components: String,

        /// Git refs for components (e.g. "pipeline:pr/123,triggers:v0.28.0")
        #[arg(long)]
        refs: Option<String>,

        /// Build components as they existed on this date (YYYY-MM-DD).
        /// Components with explicit refs ignore this.
        #[arg(long, value_parser = crate::component::validate_date_format)]
        as_of: Option<String>,

        /// After generating SNAPSHOT, trigger the standalone release-test-pipeline
        #[arg(long)]
        trigger: bool,

        /// Namespace to run the pipeline in
        #[arg(long, default_value = "ocp-midstreamer-test")]
        pipeline_namespace: String,

        /// Timeout in seconds for pipeline completion (default: 3600 = 1 hour)
        #[arg(long, default_value = "3600")]
        timeout: u64,
    },

    /// Publish test results to gh-pages branch for dashboard
    Publish {
        /// Directory containing test output (logs/ and results/ subdirs)
        #[arg(long, default_value = "./test-output")]
        output_dir: String,

        /// Git remote URL (default: origin URL of current repo)
        #[arg(long)]
        remote: Option<String>,

        /// Human-readable label for this run
        #[arg(long)]
        label: Option<String>,
    },
}
