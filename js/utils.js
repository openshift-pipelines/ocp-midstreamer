export function formatDate(isoString) {
  const d = new Date(isoString);
  return d.toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function formatDuration(secs) {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = Math.floor(secs % 60);
  const parts = [];
  if (h > 0) parts.push(`${h}h`);
  if (m > 0) parts.push(`${m}m`);
  parts.push(`${s}s`);
  return parts.join(" ");
}

export function passRate(run) {
  if (!run || run.total === 0) return 0;
  return ((run.passed / run.total) * 100).toFixed(1);
}

export const categoryColors = {
  MissingComponent: "#ffd600",
  UpgradePrereq: "#ff9100",
  UpstreamRegression: "#e94560",
  PlatformIssue: "#7c4dff",
  ConfigGap: "#00b0ff",
};
