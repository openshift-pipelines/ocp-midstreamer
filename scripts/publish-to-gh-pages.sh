#!/bin/bash
# Publish test results to gh-pages branch with full dashboard.
# This script runs AFTER streamstress completes in the Job pod.
#
# Required env vars:
#   GITHUB_TOKEN      - Token with repo write access
#   GITHUB_REPOSITORY - org/repo format
#
# Optional env vars:
#   OUTPUT_DIR     - Results directory (default: /test-output)
#   RUN_LABEL      - Label for this run (default: "CI run")
#   DASHBOARD_DIR  - Dashboard assets location (default: /dashboard or ./dashboard)

set -euo pipefail

# Check required env vars
if [ -z "${GITHUB_TOKEN:-}" ] || [ -z "${GITHUB_REPOSITORY:-}" ]; then
    echo "Publish skipped: GITHUB_TOKEN or GITHUB_REPOSITORY not set"
    exit 0
fi

OUTPUT_DIR="${OUTPUT_DIR:-/test-output}"
RUN_LABEL="${RUN_LABEL:-CI run}"
RESULTS_FILE="$OUTPUT_DIR/results/results.json"

# Find dashboard directory
if [ -n "${DASHBOARD_DIR:-}" ] && [ -d "$DASHBOARD_DIR" ]; then
    DASHBOARD_SRC="$DASHBOARD_DIR"
elif [ -d "/dashboard" ]; then
    DASHBOARD_SRC="/dashboard"
elif [ -d "./dashboard" ]; then
    DASHBOARD_SRC="./dashboard"
else
    DASHBOARD_SRC=""
fi

if [ ! -f "$RESULTS_FILE" ]; then
    echo "Publish skipped: No results file at $RESULTS_FILE"
    exit 0
fi

echo "=== Publishing results to gh-pages ==="

REPO_URL="https://x-access-token:${GITHUB_TOKEN}@github.com/${GITHUB_REPOSITORY}.git"
WORK_DIR=$(mktemp -d)
trap "rm -rf $WORK_DIR" EXIT

cd "$WORK_DIR"

# Clone gh-pages or create orphan branch
echo "Cloning gh-pages branch..."
if ! git clone --branch gh-pages --single-branch --depth 1 "$REPO_URL" . 2>/dev/null; then
    echo "gh-pages branch doesn't exist, creating..."
    git init
    git checkout --orphan gh-pages
    git remote add origin "$REPO_URL"
fi

# Configure git
git config user.name "github-actions[bot]"
git config user.email "github-actions[bot]@users.noreply.github.com"

# Copy dashboard assets if available (only on first setup or if missing)
if [ -n "$DASHBOARD_SRC" ] && [ ! -f "js/app.js" ]; then
    echo "Copying dashboard assets from $DASHBOARD_SRC..."
    cp "$DASHBOARD_SRC/index.html" .
    cp -r "$DASHBOARD_SRC/css" .
    cp -r "$DASHBOARD_SRC/js" .
fi

# Create runs directory
mkdir -p runs

# Copy results with timestamp-based filename
TIMESTAMP=$(date +%s)
RUN_FILE="${TIMESTAMP}.json"
cp "$RESULTS_FILE" "runs/${RUN_FILE}"

# Extract metadata from results for manifest entry
TOTAL=$(jq -r '.total // 0' "$RESULTS_FILE" 2>/dev/null || echo "0")
PASSED=$(jq -r '.passed // 0' "$RESULTS_FILE" 2>/dev/null || echo "0")
FAILED=$(jq -r '.failed // 0' "$RESULTS_FILE" 2>/dev/null || echo "0")
RUN_TIMESTAMP=$(jq -r '.timestamp // empty' "$RESULTS_FILE" 2>/dev/null || date -Iseconds)

# Create or update manifest.json
echo "Updating manifest.json..."
if [ -f "runs/manifest.json" ]; then
    # Append new run to existing manifest
    jq --arg id "run-${TIMESTAMP}" \
       --arg date "$RUN_TIMESTAMP" \
       --arg label "$RUN_LABEL" \
       --argjson total "$TOTAL" \
       --argjson passed "$PASSED" \
       --argjson failed "$FAILED" \
       --arg file "$RUN_FILE" \
       '.runs += [{
         id: $id,
         date: $date,
         timestamp: $date,
         label: $label,
         total: $total,
         passed: $passed,
         failed: $failed,
         file: $file
       }]' runs/manifest.json > runs/manifest.json.tmp
    mv runs/manifest.json.tmp runs/manifest.json
else
    # Create new manifest
    cat > runs/manifest.json << EOF
{
  "runs": [
    {
      "id": "run-${TIMESTAMP}",
      "date": "${RUN_TIMESTAMP}",
      "timestamp": "${RUN_TIMESTAMP}",
      "label": "${RUN_LABEL}",
      "total": ${TOTAL},
      "passed": ${PASSED},
      "failed": ${FAILED},
      "file": "${RUN_FILE}"
    }
  ]
}
EOF
fi

# Commit and push
git add -A
if git commit -m "Add results: $RUN_LABEL"; then
    echo "Pushing to gh-pages..."
    git push -u origin gh-pages

    # Extract org and repo for URL
    ORG=$(echo "$GITHUB_REPOSITORY" | cut -d'/' -f1)
    REPO=$(echo "$GITHUB_REPOSITORY" | cut -d'/' -f2)
    echo ""
    echo "✅ Results published!"
    echo "   Dashboard: https://${ORG}.github.io/${REPO}/"
else
    echo "No changes to publish."
fi
