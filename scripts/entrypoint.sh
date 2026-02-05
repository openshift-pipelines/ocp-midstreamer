#!/bin/bash
# Container entrypoint for streamstress CI Jobs.
# Runs streamstress, then publishes results if configured.

set -euo pipefail

# Run streamstress with all passed arguments
echo "Running: streamstress $*"
EXIT_CODE=0
streamstress "$@" || EXIT_CODE=$?

echo ""
echo "streamstress exited with code: $EXIT_CODE"

# Publish results if GitHub env vars are set
if [ -n "${GITHUB_TOKEN:-}" ] && [ -n "${GITHUB_REPOSITORY:-}" ]; then
    echo ""
    /usr/local/bin/publish-to-gh-pages.sh || echo "Warning: publish failed (non-fatal)"
fi

exit $EXIT_CODE
