// charts.js -- D3 chart rendering for test dashboard
// Expects D3 loaded globally via CDN in index.html

/**
 * Render a line chart showing pass rate percentage over time.
 * @param {Array} runs - Array of run objects with { date, total, passed, failed, pass_rate }
 * @param {string} containerSelector - CSS selector for chart container
 */
export function renderTrendChart(runs, containerSelector) {
  const container = d3.select(containerSelector);
  container.selectAll('*').remove();

  if (!runs || runs.length === 0) {
    container.append('p').attr('class', 'empty-state').text('No run data available for trend chart.');
    return;
  }

  const margin = { top: 20, right: 30, bottom: 40, left: 50 };
  const width = 700;
  const height = 300;
  const innerW = width - margin.left - margin.right;
  const innerH = height - margin.top - margin.bottom;

  const svg = container.append('svg')
    .attr('viewBox', `0 0 ${width} ${height}`)
    .attr('preserveAspectRatio', 'xMidYMid meet')
    .style('width', '100%');

  const g = svg.append('g')
    .attr('transform', `translate(${margin.left},${margin.top})`);

  const data = runs.map(r => ({
    date: new Date(r.date),
    passRate: r.pass_rate != null ? r.pass_rate : (r.total > 0 ? (r.passed / r.total) * 100 : 0),
    total: r.total || 0,
    passed: r.passed || 0,
    failed: r.failed || 0
  })).sort((a, b) => a.date - b.date);

  const x = d3.scaleTime()
    .domain(d3.extent(data, d => d.date))
    .range([0, innerW]);

  const y = d3.scaleLinear()
    .domain([0, 100])
    .range([innerH, 0]);

  // Grid lines
  g.append('g')
    .attr('class', 'grid')
    .call(d3.axisLeft(y).ticks(5).tickSize(-innerW).tickFormat(''))
    .selectAll('line').attr('stroke', '#333').attr('stroke-dasharray', '2,2');
  g.selectAll('.grid .domain').remove();

  // Axes
  g.append('g')
    .attr('transform', `translate(0,${innerH})`)
    .call(d3.axisBottom(x).ticks(Math.min(data.length, 8)))
    .selectAll('text,line,path').attr('stroke', '#999').attr('fill', '#999');

  g.append('g')
    .call(d3.axisLeft(y).ticks(5).tickFormat(d => d + '%'))
    .selectAll('text,line,path').attr('stroke', '#999').attr('fill', '#999');

  // Line
  const line = d3.line()
    .x(d => x(d.date))
    .y(d => y(d.passRate));

  g.append('path')
    .datum(data)
    .attr('fill', 'none')
    .attr('stroke', '#4fc3f7')
    .attr('stroke-width', 2.5)
    .attr('d', line);

  // Tooltip
  const tooltip = container.append('div')
    .attr('class', 'chart-tooltip')
    .style('position', 'absolute')
    .style('display', 'none')
    .style('background', '#1e1e2e')
    .style('border', '1px solid #4fc3f7')
    .style('padding', '8px 12px')
    .style('border-radius', '4px')
    .style('font-size', '12px')
    .style('color', '#cdd6f4')
    .style('pointer-events', 'none');

  // Dots
  g.selectAll('.dot')
    .data(data)
    .join('circle')
    .attr('class', 'dot')
    .attr('cx', d => x(d.date))
    .attr('cy', d => y(d.passRate))
    .attr('r', 4)
    .attr('fill', '#4fc3f7')
    .on('mouseenter', function (event, d) {
      tooltip.style('display', 'block')
        .html(`<strong>${d.date.toLocaleDateString()}</strong><br>Pass rate: ${d.passRate.toFixed(1)}%<br>Total: ${d.total} | Passed: ${d.passed} | Failed: ${d.failed}`);
    })
    .on('mousemove', function (event) {
      tooltip.style('left', (event.offsetX + 12) + 'px')
        .style('top', (event.offsetY - 10) + 'px');
    })
    .on('mouseleave', function () {
      tooltip.style('display', 'none');
    });
}

/**
 * Render a stacked bar chart showing failure counts by category over time.
 * @param {Array} runs - Array of run objects with { date, categories: { MissingComponent, ... } }
 * @param {string} containerSelector - CSS selector for chart container
 */
export function renderCategoryChart(runs, containerSelector) {
  const container = d3.select(containerSelector);
  container.selectAll('*').remove();

  if (!runs || runs.length === 0) {
    container.append('p').attr('class', 'empty-state').text('No run data available for category chart.');
    return;
  }

  const categoryColors = {
    MissingComponent: '#ff7043',
    UpgradePrereq: '#ffca28',
    UpstreamRegression: '#ef5350',
    PlatformIssue: '#ab47bc',
    ConfigGap: '#66bb6a'
  };
  const categories = Object.keys(categoryColors);

  const margin = { top: 20, right: 120, bottom: 40, left: 50 };
  const width = 700;
  const height = 300;
  const innerW = width - margin.left - margin.right;
  const innerH = height - margin.top - margin.bottom;

  const svg = container.append('svg')
    .attr('viewBox', `0 0 ${width} ${height}`)
    .attr('preserveAspectRatio', 'xMidYMid meet')
    .style('width', '100%');

  const g = svg.append('g')
    .attr('transform', `translate(${margin.left},${margin.top})`);

  const data = runs.map(r => {
    const row = { date: new Date(r.date), label: new Date(r.date).toLocaleDateString() };
    for (const cat of categories) {
      row[cat] = (r.categories && r.categories[cat]) || 0;
    }
    return row;
  }).sort((a, b) => a.date - b.date);

  const stack = d3.stack().keys(categories);
  const series = stack(data);

  const x = d3.scaleBand()
    .domain(data.map(d => d.label))
    .range([0, innerW])
    .padding(0.3);

  const yMax = d3.max(series, s => d3.max(s, d => d[1])) || 1;
  const y = d3.scaleLinear()
    .domain([0, yMax])
    .range([innerH, 0])
    .nice();

  // Axes
  g.append('g')
    .attr('transform', `translate(0,${innerH})`)
    .call(d3.axisBottom(x))
    .selectAll('text,line,path').attr('stroke', '#999').attr('fill', '#999');

  g.append('g')
    .call(d3.axisLeft(y).ticks(5))
    .selectAll('text,line,path').attr('stroke', '#999').attr('fill', '#999');

  // Tooltip
  const tooltip = container.append('div')
    .attr('class', 'chart-tooltip')
    .style('position', 'absolute')
    .style('display', 'none')
    .style('background', '#1e1e2e')
    .style('border', '1px solid #666')
    .style('padding', '8px 12px')
    .style('border-radius', '4px')
    .style('font-size', '12px')
    .style('color', '#cdd6f4')
    .style('pointer-events', 'none');

  // Bars
  g.selectAll('.series')
    .data(series)
    .join('g')
    .attr('class', 'series')
    .attr('fill', d => categoryColors[d.key])
    .selectAll('rect')
    .data(d => d.map(v => ({ ...v, key: d.key })))
    .join('rect')
    .attr('x', d => x(d.data.label))
    .attr('y', d => y(d[1]))
    .attr('height', d => y(d[0]) - y(d[1]))
    .attr('width', x.bandwidth())
    .on('mouseenter', function (event, d) {
      tooltip.style('display', 'block')
        .html(`<strong>${d.data.label}</strong><br>${d.key}: ${d.data[d.key]}`);
    })
    .on('mousemove', function (event) {
      tooltip.style('left', (event.offsetX + 12) + 'px')
        .style('top', (event.offsetY - 10) + 'px');
    })
    .on('mouseleave', function () {
      tooltip.style('display', 'none');
    });

  // Legend
  const legend = svg.append('g')
    .attr('transform', `translate(${width - margin.right + 10}, ${margin.top})`);

  categories.forEach((cat, i) => {
    const row = legend.append('g').attr('transform', `translate(0,${i * 20})`);
    row.append('rect').attr('width', 12).attr('height', 12).attr('fill', categoryColors[cat]);
    row.append('text').attr('x', 16).attr('y', 10).attr('fill', '#cdd6f4').attr('font-size', '11px').text(cat);
  });
}
