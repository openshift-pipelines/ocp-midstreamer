import { renderTrendChart, renderCategoryChart } from "./charts.js";
import { renderRunTables } from "./table.js";
import { renderComparison } from "./compare.js";
import { formatDate, formatDuration, passRate, categoryColors } from "./utils.js";
import { initFilters, loadFromUrl, filterRuns, filterTests, getState } from "./filters.js";

const MAX_RUNS = 10;

let _allRuns = [];

/**
 * Detect if ?sample query parameter is present.
 */
function isSampleMode() {
  return new URLSearchParams(window.location.search).has('sample');
}

/**
 * Get the base path for data files (sample-data/ or runs/).
 */
function dataBasePath() {
  return isSampleMode() ? 'sample-data/' : 'runs/';
}

/**
 * Normalize a run object to ensure consistent field names.
 * Handles format differences between publish output and dashboard expectations:
 * - timestamp -> date alias
 * - passed (bool) -> status string on tests
 * - error_message -> error alias on tests
 * - categories array -> categories object for charts
 */
function normalizeRun(run) {
  // Ensure date field exists (publish uses timestamp)
  if (!run.date && run.timestamp) {
    run.date = run.timestamp;
  }

  // Normalize categories from array [{category, count, tests}] to object {Category: count}
  if (Array.isArray(run.categories)) {
    const catObj = {};
    for (const group of run.categories) {
      const name = group.category || group.name;
      if (name) catObj[name] = group.count || 0;
    }
    run.categories = catObj;
  }

  // Normalize tests
  if (run.tests) {
    run.tests = run.tests.map(t => {
      const normalized = { ...t };
      // Convert passed bool to status string if status missing
      if (!normalized.status && typeof normalized.passed === 'boolean') {
        normalized.status = normalized.passed ? 'pass' : 'fail';
      }
      // Alias error_message -> error
      if (!normalized.error && normalized.error_message) {
        normalized.error = normalized.error_message;
      }
      // Derive category from error if missing (for failed tests from raw publish data)
      if (!normalized.category && normalized.error && normalized.status === 'fail') {
        normalized.category = inferCategory(normalized.error);
      }
      return normalized;
    });
  }

  // Ensure pass_rate
  if (run.pass_rate == null && run.total > 0) {
    run.pass_rate = (run.passed / run.total) * 100;
  }

  // Ensure duration string
  if (!run.duration && run.duration_secs) {
    const m = Math.floor(run.duration_secs / 60);
    const s = Math.floor(run.duration_secs % 60);
    run.duration = m > 0 ? m + 'm ' + s + 's' : s + 's';
  }

  return run;
}

/**
 * Simple client-side category inference matching the Rust categorize_failure logic.
 */
function inferCategory(errorMsg) {
  const lower = (errorMsg || '').toLowerCase();
  if (lower.includes('chains') || lower.includes('knative') || lower.includes('serverless') || lower.includes('manualapprovalgate')) return 'MissingComponent';
  if (lower.includes('upgrade') && (lower.includes('namespace') || lower.includes('setup') || lower.includes('prerequisite'))) return 'UpgradePrereq';
  if (lower.includes('uid_map') || (lower.includes('buildah') && lower.includes('namespace'))) return 'PlatformIssue';
  if ((lower.includes('secret') && (lower.includes('missing') || lower.includes('not found'))) || (lower.includes('auth') && (lower.includes('secret') || lower.includes('credential')))) return 'ConfigGap';
  return 'UpstreamRegression';
}

/**
 * Load individual run data files referenced in the manifest.
 */
async function loadRuns(manifest) {
  const basePath = dataBasePath();
  const entries = (manifest.runs || [])
    .map(e => {
      if (!e.date && e.timestamp) e.date = e.timestamp;
      return e;
    })
    .sort((a, b) => new Date(b.date) - new Date(a.date))
    .slice(0, MAX_RUNS);

  const runs = [];
  for (const entry of entries) {
    if (entry.file) {
      try {
        // In sample mode, file paths reference sample-data/ dir; strip 'runs/' prefix if present
        let filePath = entry.file;
        if (isSampleMode() && filePath.startsWith('runs/')) {
          filePath = filePath.replace('runs/', '');
        }
        const resp = await fetch(basePath + filePath);
        if (resp.ok) {
          const data = await resp.json();
          runs.push(normalizeRun({ ...entry, ...data }));
          continue;
        }
      } catch (_) { /* fall through to use entry as-is */ }
    }
    runs.push(normalizeRun(entry));
  }
  return runs;
}

/**
 * Ensure chart container divs exist inside #trend-charts.
 */
function ensureContainers() {
  const trendCharts = document.getElementById('trend-charts');
  if (!trendCharts) return;

  if (!document.getElementById('pass-rate-chart')) {
    const div = document.createElement('div');
    div.id = 'pass-rate-chart';
    div.style.position = 'relative';
    const h3 = document.createElement('h3');
    h3.textContent = 'Pass Rate Trend';
    trendCharts.appendChild(h3);
    trendCharts.appendChild(div);
  }

  if (!document.getElementById('category-chart')) {
    const div = document.createElement('div');
    div.id = 'category-chart';
    div.style.position = 'relative';
    const h3 = document.createElement('h3');
    h3.textContent = 'Failures by Category';
    trendCharts.appendChild(h3);
    trendCharts.appendChild(div);
  }
}

/**
 * Re-render charts and tables with current filter state applied.
 */
function renderWithFilters(state) {
  const filteredRuns = filterRuns(_allRuns, state);

  // Apply test-level filters to each run
  const runsWithFilteredTests = filteredRuns.map((run) => {
    const filtered = filterTests(run.tests || [], state);
    const passed = filtered.filter((t) => t.status === "pass" || t.status === "passed").length;
    const failed = filtered.filter((t) => t.status === "fail" || t.status === "failed").length;
    return {
      ...run,
      tests: filtered,
      passed,
      failed,
      total: filtered.length,
      pass_rate: filtered.length > 0 ? (passed / filtered.length) * 100 : 0,
    };
  });

  const descRuns = runsWithFilteredTests.slice().sort((a, b) => new Date(b.date) - new Date(a.date));
  const ascRuns = runsWithFilteredTests.slice().sort((a, b) => new Date(a.date) - new Date(b.date));

  renderTrendChart(ascRuns, '#pass-rate-chart');
  renderCategoryChart(ascRuns, '#category-chart');
  renderRunTables(descRuns, '#run-tables');
}

document.addEventListener('DOMContentLoaded', async () => {
  ensureContainers();

  try {
    const manifestPath = dataBasePath() + 'manifest.json';
    const resp = await fetch(manifestPath);
    if (!resp.ok) {
      showEmpty('No run data found. Run tests to generate data.');
      return;
    }
    const manifest = await resp.json();
    _allRuns = await loadRuns(manifest);

    // Expose run data globally for timeline module
    window.runData = _allRuns;

    if (_allRuns.length === 0) {
      showEmpty('No runs in manifest.');
      return;
    }

    // Load URL state first, then init filters with reactive callback
    const restoredState = loadFromUrl();
    initFilters(renderWithFilters);

    // Initial render
    renderWithFilters(restoredState);

    // Wire compare button
    setupCompare();

    // Setup tab switching
    setupTabs();

    console.log('Dashboard rendered with ' + _allRuns.length + ' runs');
  } catch (err) {
    console.warn('Dashboard init error:', err);
    showEmpty('Failed to load run data: ' + err.message);
  }
});

function setupCompare() {
  const btn = document.getElementById('btn-compare');
  const selA = document.getElementById('compare-run-a');
  const selB = document.getElementById('compare-run-b');
  if (!btn || !selA || !selB) return;

  // Populate dropdowns
  _allRuns.forEach((run, i) => {
    const label = new Date(run.date).toLocaleDateString() + ' (' + (run.passed || 0) + '/' + (run.total || 0) + ')';
    const optA = document.createElement('option');
    optA.value = i;
    optA.textContent = label;
    selA.appendChild(optA);
    const optB = document.createElement('option');
    optB.value = i;
    optB.textContent = label;
    selB.appendChild(optB);
  });

  if (_allRuns.length >= 2) {
    selA.value = 1;
    selB.value = 0;
  }

  btn.addEventListener('click', () => {
    const a = _allRuns[parseInt(selA.value)];
    const b = _allRuns[parseInt(selB.value)];
    if (a && b) {
      renderComparison(a, b, '#compare-view');
    }
  });
}

function showEmpty(msg) {
  const el = document.getElementById('run-tables') || document.getElementById('trend-charts');
  if (el) {
    const p = document.createElement('p');
    p.className = 'empty-state';
    p.textContent = msg;
    el.appendChild(p);
  }
}

/**
 * Setup tab navigation functionality.
 */
function setupTabs() {
  const tabButtons = document.querySelectorAll('.tab-btn');
  const tabContents = document.querySelectorAll('.tab-content');

  tabButtons.forEach(btn => {
    btn.addEventListener('click', () => {
      const tabName = btn.dataset.tab;

      // Update button states
      tabButtons.forEach(b => b.classList.remove('active'));
      btn.classList.add('active');

      // Show/hide tab content
      tabContents.forEach(content => {
        if (content.id === tabName + '-view') {
          content.style.display = 'block';
        } else {
          content.style.display = 'none';
        }
      });

      // Load timeline data when switching to timeline tab
      if (tabName === 'timeline') {
        // Update global runData in case it changed
        window.runData = _allRuns;
        // Trigger timeline module to load data
        if (window.timelineModule) {
          window.timelineModule.loadTimelineData();
        }
      }
    });
  });
}
