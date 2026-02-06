#!/bin/bash
# Container entrypoint for streamstress CI Jobs.
# Runs streamstress, then publishes results if configured.

set -euo pipefail

# Create kubeconfig from ServiceAccount token so that tools like gauge's
# release-tests (which use client-go and expect ~/.kube/config) can access
# the cluster. The SA token is auto-mounted by Kubernetes.
SA_DIR="/var/run/secrets/kubernetes.io/serviceaccount"
if [ -f "$SA_DIR/token" ] && [ -n "${KUBERNETES_SERVICE_HOST:-}" ]; then
    APISERVER="https://${KUBERNETES_SERVICE_HOST}:${KUBERNETES_SERVICE_PORT}"
    TOKEN=$(cat "$SA_DIR/token")
    mkdir -p /root/.kube
    cat > /root/.kube/config <<KUBEEOF
apiVersion: v1
kind: Config
clusters:
- cluster:
    certificate-authority: ${SA_DIR}/ca.crt
    server: ${APISERVER}
  name: in-cluster
contexts:
- context:
    cluster: in-cluster
    user: sa-user
  name: in-cluster
current-context: in-cluster
users:
- name: sa-user
  user:
    token: ${TOKEN}
KUBEEOF
    echo "Created kubeconfig from ServiceAccount token"
fi

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
