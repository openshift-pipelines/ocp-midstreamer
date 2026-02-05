# streamstress

Detect upstream Tekton changes that break OpenShift Pipelines — before they reach the midstream.

`streamstress` is a Rust CLI that builds upstream Tekton components from source, swaps the resulting images into a live OpenShift Pipelines operator deployment, and runs the [release-tests](https://github.com/openshift-pipelines/release-tests) suite to catch regressions early.

## What It Does

The tool automates a workflow that would otherwise require many manual steps:

1. **Auto-Setup** — Enables the OCP internal image registry route, installs the OpenShift Pipelines operator via OLM if missing, creates the target namespace with image-pull RBAC. All idempotent — safe to run on already-configured clusters.

2. **Build** — Shallow-clones upstream Tekton repos, builds container images using `ko` (or `docker` for console-plugin), and pushes them to the OCP internal registry under the `tekton-upstream` namespace. Multiple components build in parallel via tokio.

3. **Deploy** — Finds the OpenShift Pipelines operator's ClusterServiceVersion (CSV), patches `IMAGE_*` environment variables to point at the upstream-built images (using the internal `image-registry.openshift-image-registry.svc:5000` address), then deletes the relevant TektonInstallerSets to force the operator to re-reconcile with the new images. Waits for TektonConfig Ready condition and verifies pod images match.

4. **Test** — Clones the `release-tests` repo, runs Gauge specs with streaming output (piped to both terminal and log files), parses results from JUnit XML (or falls back to Gauge stdout parsing when XML is unavailable), and categorizes failures.

5. **Report** — Writes structured JSON results with per-test pass/fail, duration, error messages, and failure categorization. Optionally includes per-spec resource profiles (CPU, memory, pod count) for parallelism planning.

6. **Publish** — Pushes results to a `gh-pages` branch with a self-contained HTML dashboard for trend analysis across runs.

### Why CSV Patching?

OLM (Operator Lifecycle Manager) owns the operator Deployments. Direct Deployment patches get reverted. The correct approach is to patch the CSV, which OLM then propagates to Deployments. After patching, InstallerSets are deleted to force the operator controller to re-create them with the new image references.

### Why Internal Registry URL?

Pods running in the cluster can't authenticate to the external registry route. The `IMAGE_*` env vars use the internal service address (`svc:5000`) so that operator-managed pods can pull the upstream-built images without extra auth configuration.

## Supported Components

| Component | Upstream Repo | Build | InstallerSet Prefix |
|-----------|--------------|-------|---------------------|
| pipeline | [tektoncd/pipeline](https://github.com/tektoncd/pipeline) | ko | `pipeline` |
| triggers | [tektoncd/triggers](https://github.com/tektoncd/triggers) | ko | `trigger` |
| chains | [tektoncd/chains](https://github.com/tektoncd/chains) | ko | `chain` |
| results | [tektoncd/results](https://github.com/tektoncd/results) | ko | `result` |
| manual-approval-gate | [openshift-pipelines/manual-approval-gate](https://github.com/openshift-pipelines/manual-approval-gate) | ko | `manualapprovalgate` |
| console-plugin | [openshift-pipelines/console-plugin](https://github.com/openshift-pipelines/console-plugin) | docker | `tekton-config-console-plugin-manifests` |

Component configuration lives in `config/components.toml` — each entry maps upstream repo URLs, ko import paths, and `IMAGE_*` env var names used by the operator.

## Prerequisites

- An OpenShift 4.x cluster with cluster-admin access
- `oc`, `ko`, `git`, `go`, `gauge` (with go and xml-report plugins)
- Rust toolchain (for building the CLI)

> The CLI auto-enables the registry route and installs the OpenShift Pipelines operator if missing. Pass `--no-auto-setup` to skip this.

## Usage

```bash
# Build the CLI
cargo build --release

# Check prerequisites and cluster connectivity
streamstress check

# Full build → deploy → test for one component
streamstress run --components pipeline

# All components
streamstress run --components pipeline,triggers,chains,results,manual-approval-gate

# Pin specific git refs (branch, tag, PR, or commit)
streamstress run --components "pipeline:v0.62.0,triggers:pr/123"

# Dry run — resolve refs, show plan, don't execute
streamstress run --components pipeline,triggers --dry-run
streamstress run --components pipeline --dry-run --json

# With resource profiling (metrics-server required)
streamstress run --components pipeline --profile

# Individual stages
streamstress build --component pipeline
streamstress deploy --component pipeline --registry <registry-route>/tekton-upstream
streamstress test --release-tests-ref master

# Re-analyze past results
streamstress results --output-dir ./test-output

# In-cluster Job management
streamstress status
streamstress logs
streamstress logs --job streamstress-1706900000

# Publish results to dashboard
streamstress publish --label "upstream pipeline @ main"
```

## Subcommands

| Command | Description |
|---------|-------------|
| `check` | Verify tool prerequisites (oc, ko, git, go), cluster auth, operator, registry. Shows `[auto-fixable]` for items that auto-setup can resolve. |
| `build` | Clone upstream repo, build images with ko/docker, push to OCP internal registry. |
| `deploy` | Patch operator CSV with upstream image refs, delete InstallerSets, wait for reconciliation. |
| `test` | Clone release-tests, run Gauge specs, parse JUnit XML or stdout, categorize failures. |
| `run` | Full orchestration: auto-setup → parallel builds → in-cluster Job for deploy+test. |
| `results` | Offline re-analysis of a previous test run's output directory. |
| `status` | List streamstress Jobs in the cluster with status and age. |
| `logs` | Stream logs from the most recent (or named) Job pod. |
| `publish` | Push results JSON + dashboard assets to gh-pages orphan branch. |

## Execution Modes

### Local Build + In-Cluster Deploy/Test (default for `run`)

```
local machine                          cluster
─────────────                          ───────
auto-setup ──────────────────────────► registry route, operator, RBAC
parallel ko builds ──────────────────► push images to internal registry
create Job (--skip-build) ───────────► Job deploys + tests
status / logs ◄──────────────────────  stream results back
```

The `run` subcommand builds images locally (parallel via tokio JoinSet), then creates a Kubernetes Job that runs the deploy+test phases in-cluster. The Job uses a cached CLI container image (rebuilt only on version bumps). Use `status` and `logs` to monitor.

### Fully Local (individual subcommands)

Run `build`, `deploy`, `test` separately for full local control.

## Resource Profiling

With `--profile`, the CLI collects per-spec resource usage via the metrics-server API:

- Polls `PodMetrics` during test execution
- Detects spec boundaries in Gauge output
- Computes min/max/avg/p95 CPU and memory per spec
- Calculates maximum safe parallelism: `(allocatable - baseline) × 80% / peak_per_spec`
- Reports limiting resource (CPU vs memory) and reasoning

Output written to `test-output/results/resource-profile.json`.

## Failure Categories

Test failures are automatically categorized by keyword matching:

| Category | Trigger | Meaning |
|----------|---------|---------|
| Missing Optional Components | "chains", "manual-approval", "knative" | Test needs a component not deployed |
| Upgrade Prerequisites | "upgrade" + "namespace"/"setup" | Test assumes an upgrade path |
| Upstream Regression | (default) | Likely upstream code change |
| Platform Issue | "uid_map", "buildah+namespace" | Cluster/infra problem |
| Configuration Gap | missing secrets/auth | Missing setup or RBAC |

## CI/CD

GitHub Actions workflow at `.github/workflows/streamstress-run.yml`:

```bash
gh workflow run streamstress-run.yml \
  -f cluster_api_url="https://api.cluster.example.com:6443" \
  -f cluster_password="..." \
  -f components="pipeline,triggers,chains,results,manual-approval-gate" \
  -f release_tests_ref="master" \
  -f cli_flags="--profile"
```

The workflow installs all tools, builds the CLI, authenticates to the cluster and registry, runs the full cycle, and publishes results to GitHub Pages.

## Dashboard

Published via `streamstress publish`, the dashboard provides:

- D3.js trend charts (pass/fail rates across runs)
- Per-test expandable result tables
- Failure categorization breakdown
- Run comparison and regression highlighting
- Reactive filters with URL state persistence
- Resource usage overlay (when profiling data available)

View at `https://<org>.github.io/<repo>/` after publishing.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | All tests passed |
| 1 | Some tests failed |
| 2 | Build or infrastructure error |
| 3 | Both build error and test failures |

## License

Apache-2.0
