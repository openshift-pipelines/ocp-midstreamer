// compare.js -- Side-by-side run comparison

/**
 * Render a comparison between two runs showing new failures, fixed, and unchanged.
 * @param {Object} runA - First run (typically older)
 * @param {Object} runB - Second run (typically newer)
 * @param {string} containerSelector - CSS selector for comparison container
 */
export function renderComparison(runA, runB, containerSelector) {
  const container = document.querySelector(containerSelector);
  if (!container) return;
  container.replaceChildren();

  const testsA = buildTestMap(runA.tests || []);
  const testsB = buildTestMap(runB.tests || []);

  const newFailures = [];
  const fixed = [];
  const unchangedFailures = [];

  // Check all tests in run B
  for (const [key, testB] of testsB) {
    const testA = testsA.get(key);
    const bFailed = isFailed(testB);
    const aFailed = testA ? isFailed(testA) : false;
    const aPassed = testA ? !isFailed(testA) : false;

    if (bFailed && (!testA || aPassed)) {
      newFailures.push(testB);
    } else if (bFailed && aFailed) {
      unchangedFailures.push(testB);
    }
  }

  // Check tests in run A that passed->fixed in B
  for (const [key, testA] of testsA) {
    const testB = testsB.get(key);
    if (isFailed(testA) && testB && !isFailed(testB)) {
      fixed.push(testB);
    }
  }

  // Render header
  const header = document.createElement('h3');
  header.textContent = 'Comparison: ' + formatRunLabel(runA) + ' vs ' + formatRunLabel(runB);
  container.appendChild(header);

  // Render sections
  renderSection(container, 'New Failures', newFailures, 'compare-new-failures');
  renderSection(container, 'Fixed', fixed, 'compare-fixed');
  renderSection(container, 'Unchanged Failures', unchangedFailures, 'compare-unchanged');

  if (newFailures.length === 0 && fixed.length === 0 && unchangedFailures.length === 0) {
    const p = document.createElement('p');
    p.className = 'empty-state';
    p.textContent = 'No test differences found between these runs.';
    container.appendChild(p);
  }
}

function buildTestMap(tests) {
  const map = new Map();
  for (const t of tests) {
    const key = (t.spec || '') + '::' + (t.scenario || t.name || '');
    map.set(key, t);
  }
  return map;
}

function isFailed(test) {
  const s = (test.status || '').toLowerCase();
  return s === 'fail' || s === 'failed';
}

function formatRunLabel(run) {
  return new Date(run.date).toLocaleDateString();
}

function renderSection(container, title, tests, className) {
  const section = document.createElement('div');
  section.className = 'compare-section ' + className;

  const h4 = document.createElement('h4');
  h4.textContent = title + ' (' + tests.length + ')';
  section.appendChild(h4);

  if (tests.length === 0) {
    const p = document.createElement('p');
    p.className = 'empty-state';
    p.textContent = 'None';
    section.appendChild(p);
  } else {
    const ul = document.createElement('ul');
    tests.forEach((t) => {
      const li = document.createElement('li');
      li.textContent = (t.spec || '') + ' / ' + (t.scenario || t.name || '');
      if (t.category) {
        const badge = document.createElement('span');
        badge.className = 'cat-badge';
        badge.textContent = t.category;
        li.appendChild(badge);
      }
      ul.appendChild(li);
    });
    section.appendChild(ul);
  }

  container.appendChild(section);
}
