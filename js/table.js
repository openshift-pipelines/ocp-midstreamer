// table.js -- Expandable per-run detail tables with detail panel and regression highlighting

/**
 * Compute regressions by comparing current run against previous run.
 * Returns a Map of test key -> 'regression' | 'fixed'
 */
export function computeRegressions(currentRun, previousRun) {
  const result = new Map();
  if (!previousRun || !previousRun.tests) return result;

  const prevMap = new Map();
  for (const t of previousRun.tests) {
    const key = (t.spec || '') + '::' + (t.scenario || t.name || '');
    prevMap.set(key, t);
  }

  for (const t of (currentRun.tests || [])) {
    const key = (t.spec || '') + '::' + (t.scenario || t.name || '');
    const prev = prevMap.get(key);
    if (!prev) continue;

    const curFail = isFail(t);
    const prevFail = isFail(prev);

    if (curFail && !prevFail) {
      result.set(key, 'regression');
    } else if (!curFail && prevFail) {
      result.set(key, 'fixed');
    }
  }

  return result;
}

function isFail(t) {
  const s = (t.status || '').toLowerCase();
  return s === 'fail' || s === 'failed';
}

/**
 * Show detail panel for a test.
 */
function showDetailPanel(test) {
  const panel = document.getElementById('detail-panel');
  if (!panel) return;

  panel.replaceChildren();
  panel.classList.remove('hidden');

  const closeBtn = document.createElement('button');
  closeBtn.className = 'detail-close';
  closeBtn.textContent = 'Close';
  closeBtn.addEventListener('click', () => panel.classList.add('hidden'));
  panel.appendChild(closeBtn);

  const h3 = document.createElement('h3');
  h3.textContent = test.scenario || test.name || 'Test Detail';
  panel.appendChild(h3);

  const fields = [
    ['Spec', test.spec],
    ['Scenario', test.scenario || test.name],
    ['Status', (test.status || 'unknown').toUpperCase()],
    ['Duration', test.duration || '-'],
    ['Category', test.category || '-'],
  ];

  fields.forEach(([label, value]) => {
    const row = document.createElement('div');
    row.className = 'detail-field';
    const lbl = document.createElement('span');
    lbl.className = 'detail-label';
    lbl.textContent = label + ':';
    const val = document.createElement('span');
    val.className = 'detail-value';
    val.textContent = value || '-';
    row.appendChild(lbl);
    row.appendChild(val);
    panel.appendChild(row);
  });

  // Error message
  if (test.error || test.message) {
    const errLabel = document.createElement('h4');
    errLabel.textContent = 'Error Message';
    errLabel.style.marginTop = '1rem';
    panel.appendChild(errLabel);

    const pre = document.createElement('pre');
    pre.className = 'detail-error';
    pre.textContent = test.error || test.message || '';
    panel.appendChild(pre);
  }
}

/**
 * Render collapsible run detail cards with test result tables.
 * @param {Array} runs - Array of run objects (sorted descending by date)
 * @param {string} containerSelector - CSS selector for container
 */
export function renderRunTables(runs, containerSelector) {
  const container = document.querySelector(containerSelector);
  if (!container) return;
  container.replaceChildren();

  if (!runs || runs.length === 0) {
    const p = document.createElement('p');
    p.className = 'empty-state';
    p.textContent = 'No run data available.';
    container.appendChild(p);
    return;
  }

  runs.forEach((run, index) => {
    // Previous run for regression detection (runs are desc sorted, so previous = index+1)
    const previousRun = index < runs.length - 1 ? runs[index + 1] : null;
    const regressions = computeRegressions(run, previousRun);

    const card = document.createElement('div');
    card.className = 'run-card';

    const passRate = run.pass_rate != null
      ? run.pass_rate
      : (run.total > 0 ? ((run.passed / run.total) * 100) : 0);

    const badgeClass = passRate >= 80 ? 'badge-pass' : passRate >= 50 ? 'badge-warn' : 'badge-fail';

    // Header
    const header = document.createElement('div');
    header.className = 'run-card-header';

    const dateSpan = document.createElement('span');
    dateSpan.className = 'run-date';
    dateSpan.textContent = new Date(run.date).toLocaleDateString() + ' ' + new Date(run.date).toLocaleTimeString();

    const badge = document.createElement('span');
    badge.className = 'badge ' + badgeClass;
    badge.textContent = passRate.toFixed(1) + '%';

    const counts = document.createElement('span');
    counts.className = 'run-counts';
    counts.textContent = (run.passed || 0) + ' passed / ' + (run.failed || 0) + ' failed / ' + (run.total || 0) + ' total';

    const expandIcon = document.createElement('span');
    expandIcon.className = 'expand-icon';
    expandIcon.textContent = index === 0 ? '\u25BC' : '\u25B6';

    header.appendChild(dateSpan);
    header.appendChild(badge);
    header.appendChild(counts);

    if (run.duration) {
      const dur = document.createElement('span');
      dur.className = 'run-duration';
      dur.textContent = run.duration;
      header.appendChild(dur);
    }

    header.appendChild(expandIcon);

    // Detail body
    const body = document.createElement('div');
    body.className = 'run-card-body';
    body.style.display = index === 0 ? 'block' : 'none';

    const tests = (run.tests || []).slice().sort((a, b) => {
      if (a.status === 'fail' && b.status !== 'fail') return -1;
      if (a.status !== 'fail' && b.status === 'fail') return 1;
      return 0;
    });

    if (tests.length === 0) {
      const emptyP = document.createElement('p');
      emptyP.className = 'empty-state';
      emptyP.textContent = 'No test details available for this run.';
      body.appendChild(emptyP);
    } else {
      const table = document.createElement('table');
      table.className = 'run-detail-table';

      const thead = document.createElement('thead');
      const headRow = document.createElement('tr');
      ['Status', 'Spec', 'Scenario', 'Duration', 'Category'].forEach(col => {
        const th = document.createElement('th');
        th.textContent = col;
        headRow.appendChild(th);
      });
      thead.appendChild(headRow);
      table.appendChild(thead);

      const tbody = document.createElement('tbody');
      tests.forEach(t => {
        const key = (t.spec || '') + '::' + (t.scenario || t.name || '');
        const regStatus = regressions.get(key);

        const tr = document.createElement('tr');
        tr.className = 'status-' + (t.status || 'unknown');
        if (regStatus === 'regression') tr.classList.add('regression-row');
        if (regStatus === 'fixed') tr.classList.add('fixed-row');

        // Click to open detail panel
        tr.style.cursor = 'pointer';
        tr.addEventListener('click', () => showDetailPanel(t));

        const statusTd = document.createElement('td');
        const statusSpan = document.createElement('span');
        statusSpan.className = 'status-badge status-' + (t.status || 'unknown');
        statusSpan.textContent = (t.status || 'unknown').toUpperCase();
        statusTd.appendChild(statusSpan);

        if (regStatus === 'regression') {
          const regBadge = document.createElement('span');
          regBadge.className = 'regression-badge';
          regBadge.textContent = 'REGRESSION';
          statusTd.appendChild(regBadge);
        } else if (regStatus === 'fixed') {
          const fixBadge = document.createElement('span');
          fixBadge.className = 'fixed-badge';
          fixBadge.textContent = 'FIXED';
          statusTd.appendChild(fixBadge);
        }

        tr.appendChild(statusTd);

        const specTd = document.createElement('td');
        specTd.textContent = t.spec || '-';
        tr.appendChild(specTd);

        const scenarioTd = document.createElement('td');
        scenarioTd.textContent = t.scenario || t.name || '-';
        tr.appendChild(scenarioTd);

        const durTd = document.createElement('td');
        durTd.textContent = t.duration || '-';
        tr.appendChild(durTd);

        const catTd = document.createElement('td');
        catTd.textContent = t.category || '-';
        tr.appendChild(catTd);

        tbody.appendChild(tr);
      });
      table.appendChild(tbody);
      body.appendChild(table);
    }

    header.addEventListener('click', () => {
      const visible = body.style.display === 'block';
      body.style.display = visible ? 'none' : 'block';
      expandIcon.textContent = visible ? '\u25B6' : '\u25BC';
    });

    card.appendChild(header);
    card.appendChild(body);
    container.appendChild(card);
  });
}
