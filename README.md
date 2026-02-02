# ocp-midstreamer

Detect upstream Tekton changes that break OpenShift Pipelines — before they reach the midstream.

`ocp-midstreamer` builds upstream Tekton components from source, swaps the images into a live OpenShift Pipelines operator deployment via TektonConfig CR patching, and runs the [release-tests](https://github.com/openshift-pipelines/release-tests) suite to catch breakages early.

## Supported Components

| Component | Upstream Repo | Build System |
|-----------|--------------|--------------|
| pipeline | tektoncd/pipeline | ko |
| triggers | tektoncd/triggers | ko |
| chains | tektoncd/chains | ko |
| results | tektoncd/results | ko |
| manual-approval-gate | openshift-pipelines/manual-approval-gate | ko |
| console-plugin | openshift-pipelines/console-plugin | docker |

## Prerequisites

- An OpenShift cluster (4.x) with cluster-admin access
- `oc`, `ko`, `git`, `go`, `gauge` installed locally (or use CI)
- Rust toolchain (for building the CLI)

> **Auto-setup:** The CLI automatically enables the internal registry route and installs the OpenShift Pipelines operator if they're missing. Use `--no-auto-setup` to disable this.

## Quick Start

```bash
# Build the CLI
cargo build --release

# Check prerequisites
./target/release/ocp-midstreamer check

# Run the full cycle for pipeline component
./target/release/ocp-midstreamer run --components pipeline

# Run with all components
./target/release/ocp-midstreamer run --components pipeline,triggers,chains,results,manual-approval-gate

# Custom git refs per component
./target/release/ocp-midstreamer run --components "pipeline:v0.62.0,triggers:pr/123"

# Dry run — see the plan without executing
./target/release/ocp-midstreamer run --components pipeline,triggers --dry-run

# With resource profiling
./target/release/ocp-midstreamer run --components pipeline --profile
```

## Subcommands

| Command | Description |
|---------|-------------|
| `check` | Verify tool prerequisites and cluster connectivity |
| `build` | Build a single component's images with ko |
| `deploy` | Patch operator to use upstream-built images |
| `test` | Run release-tests Gauge suite |
| `run` | Full build → deploy → test cycle (multi-component) |
| `results` | Re-analyze results from a previous test run |
| `status` | Show status of in-cluster midstreamer Jobs |
| `logs` | Stream logs from a midstreamer Job pod |
| `publish` | Push test results to gh-pages for the dashboard |

## How It Works

1. **Build** — Clones upstream Tekton repos, builds container images with `ko`, pushes to the OCP internal registry
2. **Deploy** — Finds the OpenShift Pipelines operator CSV, patches `IMAGE_*` env vars to point at upstream-built images, deletes InstallerSets to force re-reconciliation
3. **Test** — Clones `release-tests`, runs Gauge specs, parses JUnit XML (or falls back to stdout parsing), categorizes failures
4. **Report** — Writes structured JSON results, categorized failure breakdown, and optional resource profiles

## CI/CD

A GitHub Actions workflow is included at `.github/workflows/midstreamer-run.yml`. It accepts:

- `cluster_api_url` — OCP cluster API endpoint
- `cluster_password` — cluster-admin password
- `components` — comma-separated component list
- `release_tests_ref` — release-tests git ref
- `cli_flags` — extra flags (e.g. `--profile`, `--dry-run`)

Trigger manually from the Actions tab or via:

```bash
gh workflow run midstreamer-run.yml \
  -f cluster_api_url="https://api.cluster.example.com:6443" \
  -f cluster_password="..." \
  -f components="pipeline,triggers,chains,results,manual-approval-gate"
```

Results are published to GitHub Pages as an interactive dashboard.

## Dashboard

After a test run, publish results to an HTML dashboard:

```bash
./target/release/ocp-midstreamer publish --label "my test run"
```

The dashboard includes:
- D3.js trend charts across runs
- Per-test pass/fail breakdown
- Failure categorization (missing components, upgrade prereqs, upstream regressions, platform issues, config gaps)
- Run comparison and regression highlighting
- Reactive filters with URL state

## Failure Categories

Test failures are automatically categorized:

| Category | Meaning |
|----------|---------|
| Missing Optional Components | Test requires a component not deployed |
| Upgrade Prerequisites | Test assumes upgrade path not present |
| Upstream Regression | Likely upstream code change broke behavior |
| Platform Issue | Cluster/infra problem, not code |
| Configuration Gap | Missing config, RBAC, or setup |

## License

Apache-2.0
