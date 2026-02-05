// Timeline visualization for historical nightly runs
// Uses D3.js for date-based charting
// NOTE: Uses safe DOM APIs (textContent, createElement) instead of innerHTML for security

(function() {
    'use strict';

    // Configuration
    const CHART_MARGIN = { top: 20, right: 80, bottom: 50, left: 60 };
    const CHART_HEIGHT = 400;

    // State
    let timelineData = [];
    let filteredData = [];
    let showPerf = false;
    let showResources = false;
    let highlightRegressions = true;

    // Initialize when DOM is ready
    document.addEventListener('DOMContentLoaded', function() {
        initTimelineControls();
    });

    function initTimelineControls() {
        // Date range selector
        document.getElementById('apply-date-range')?.addEventListener('click', applyDateRange);

        // Toggles
        document.getElementById('show-perf-overlay')?.addEventListener('change', function(e) {
            showPerf = e.target.checked;
            renderTimeline();
        });
        document.getElementById('show-resources')?.addEventListener('change', function(e) {
            showResources = e.target.checked;
            renderTimeline();
        });
        document.getElementById('highlight-regressions')?.addEventListener('change', function(e) {
            highlightRegressions = e.target.checked;
            renderTimeline();
        });

        // Export
        document.getElementById('export-timeline-csv')?.addEventListener('click', exportTimelineCSV);

        // Tab activation - listen for tab clicks
        document.querySelectorAll('[data-tab="timeline"]').forEach(btn => {
            btn.addEventListener('click', function() {
                loadTimelineData();
            });
        });
    }

    // Load and process run data for timeline
    function loadTimelineData() {
        if (!window.runData || !Array.isArray(window.runData)) {
            console.warn('No run data available for timeline');
            return;
        }

        // Filter to runs with as_of_date (historical runs) or use date field
        timelineData = window.runData
            .filter(run => run.as_of_date || run.date)
            .map(run => ({
                date: new Date(run.as_of_date || run.date),
                dateStr: run.as_of_date || run.date?.split('T')[0] || '',
                passRate: calculatePassRate(run),
                totalTests: run.summary?.total || run.total || 0,
                passed: run.summary?.passed || run.passed || 0,
                failed: run.summary?.failed || run.failed || 0,
                commitSha: run.component_refs?.[0]?.sha || run.commit_sha || 'N/A',
                commitMessage: run.component_refs?.[0]?.message || '',
                performance: run.performance || null,
                resources: run.performance_resources || run.resource_profile || null,
                run: run
            }))
            .sort((a, b) => a.date - b.date);

        // Set default date range
        if (timelineData.length > 0) {
            const startInput = document.getElementById('timeline-start');
            const endInput = document.getElementById('timeline-end');
            if (startInput && endInput) {
                startInput.value = timelineData[0].dateStr.split('T')[0];
                endInput.value = timelineData[timelineData.length - 1].dateStr.split('T')[0];
            }
        }

        filteredData = [...timelineData];
        renderTimeline();
    }

    function calculatePassRate(run) {
        const total = run.summary?.total || run.total || 0;
        const passed = run.summary?.passed || run.passed || 0;
        if (total === 0) return 0;
        return (passed / total) * 100;
    }

    function applyDateRange() {
        const startStr = document.getElementById('timeline-start')?.value;
        const endStr = document.getElementById('timeline-end')?.value;

        if (!startStr || !endStr) return;

        const startDate = new Date(startStr);
        const endDate = new Date(endStr);
        endDate.setHours(23, 59, 59);

        filteredData = timelineData.filter(d => d.date >= startDate && d.date <= endDate);
        renderTimeline();
    }

    function renderTimeline() {
        const container = document.getElementById('timeline-chart');
        if (!container) return;

        // Clear previous chart using DOM API
        while (container.firstChild) {
            container.removeChild(container.firstChild);
        }

        if (filteredData.length === 0) {
            const noDataMsg = document.createElement('p');
            noDataMsg.className = 'no-data';
            noDataMsg.textContent = 'No historical run data available. Use --date-range to generate historical runs.';
            container.appendChild(noDataMsg);
            return;
        }

        const width = container.clientWidth || 900;
        const height = CHART_HEIGHT;
        const innerWidth = width - CHART_MARGIN.left - CHART_MARGIN.right;
        const innerHeight = height - CHART_MARGIN.top - CHART_MARGIN.bottom;

        // Create SVG
        const svg = d3.select(container)
            .append('svg')
            .attr('width', width)
            .attr('height', height);

        const g = svg.append('g')
            .attr('transform', `translate(${CHART_MARGIN.left},${CHART_MARGIN.top})`);

        // Scales
        const xScale = d3.scaleTime()
            .domain(d3.extent(filteredData, d => d.date))
            .range([0, innerWidth]);

        const yScale = d3.scaleLinear()
            .domain([0, 100])
            .range([innerHeight, 0]);

        // Axes
        const tickInterval = Math.max(1, Math.ceil(filteredData.length / 10));
        const xAxis = d3.axisBottom(xScale)
            .ticks(d3.timeDay.every(tickInterval))
            .tickFormat(d3.timeFormat('%m/%d'));

        const yAxis = d3.axisLeft(yScale)
            .ticks(5)
            .tickFormat(d => d + '%');

        g.append('g')
            .attr('class', 'x-axis')
            .attr('transform', `translate(0,${innerHeight})`)
            .call(xAxis)
            .selectAll('text')
            .attr('transform', 'rotate(-45)')
            .style('text-anchor', 'end');

        g.append('g')
            .attr('class', 'y-axis')
            .call(yAxis);

        // Y-axis label
        g.append('text')
            .attr('class', 'axis-label')
            .attr('transform', 'rotate(-90)')
            .attr('y', -45)
            .attr('x', -innerHeight / 2)
            .attr('text-anchor', 'middle')
            .text('Pass Rate (%)');

        // Line generator
        const line = d3.line()
            .x(d => xScale(d.date))
            .y(d => yScale(d.passRate))
            .curve(d3.curveMonotoneX);

        // Draw pass rate line
        g.append('path')
            .datum(filteredData)
            .attr('class', 'timeline-line')
            .attr('fill', 'none')
            .attr('stroke', '#4CAF50')
            .attr('stroke-width', 2)
            .attr('d', line);

        // Data points
        g.selectAll('.data-point')
            .data(filteredData)
            .enter()
            .append('circle')
            .attr('class', 'data-point')
            .attr('cx', d => xScale(d.date))
            .attr('cy', d => yScale(d.passRate))
            .attr('r', 5)
            .attr('fill', d => getPointColor(d))
            .attr('stroke', '#fff')
            .attr('stroke-width', 1)
            .style('cursor', 'pointer')
            .on('click', (event, d) => showRunDetail(d))
            .on('mouseover', (event, d) => showTooltip(event, d))
            .on('mouseout', hideTooltip);

        // Highlight regressions
        if (highlightRegressions) {
            highlightRegressionPoints(g, xScale, yScale);
        }

        // Performance overlay
        if (showPerf) {
            renderPerfOverlay(g, xScale, innerHeight, innerWidth);
        }

        // Resource usage overlay
        if (showResources) {
            renderResourceOverlay(g, xScale, innerHeight, innerWidth);
        }

        // Legend
        renderLegend();
    }

    function getPointColor(d) {
        if (d.passRate === 100) return '#4CAF50';
        if (d.passRate >= 90) return '#8BC34A';
        if (d.passRate >= 70) return '#FFC107';
        if (d.passRate >= 50) return '#FF9800';
        return '#F44336';
    }

    function highlightRegressionPoints(g, xScale, yScale) {
        const regressions = [];
        for (let i = 1; i < filteredData.length; i++) {
            const prev = filteredData[i - 1];
            const curr = filteredData[i];
            if (prev.passRate - curr.passRate > 10) {
                regressions.push({
                    date: curr.date,
                    from: prev.passRate,
                    to: curr.passRate,
                    data: curr
                });
            }
        }

        g.selectAll('.regression-marker')
            .data(regressions)
            .enter()
            .append('circle')
            .attr('class', 'regression-marker')
            .attr('cx', d => xScale(d.date))
            .attr('cy', d => yScale(d.to))
            .attr('r', 12)
            .attr('fill', 'none')
            .attr('stroke', '#F44336')
            .attr('stroke-width', 2)
            .attr('stroke-dasharray', '4,2');

        g.selectAll('.regression-label')
            .data(regressions)
            .enter()
            .append('text')
            .attr('class', 'regression-label')
            .attr('x', d => xScale(d.date))
            .attr('y', d => yScale(d.to) - 18)
            .attr('text-anchor', 'middle')
            .attr('fill', '#F44336')
            .attr('font-size', '11px')
            .text(d => '\u2193' + (d.from - d.to).toFixed(0) + '%');
    }

    function renderPerfOverlay(g, xScale, innerHeight, innerWidth) {
        const perfData = filteredData.filter(d => d.performance?.metrics?.throughput_per_minute);
        if (perfData.length === 0) return;

        const maxThroughput = d3.max(perfData, d => d.performance.metrics.throughput_per_minute) || 100;
        const perfYScale = d3.scaleLinear()
            .domain([0, maxThroughput * 1.1])
            .range([innerHeight, 0]);

        const perfYAxis = d3.axisRight(perfYScale)
            .ticks(5)
            .tickFormat(d => d.toFixed(0) + '/min');

        g.append('g')
            .attr('class', 'perf-y-axis')
            .attr('transform', `translate(${innerWidth},0)`)
            .call(perfYAxis);

        const perfLine = d3.line()
            .x(d => xScale(d.date))
            .y(d => perfYScale(d.performance.metrics.throughput_per_minute))
            .curve(d3.curveMonotoneX);

        g.append('path')
            .datum(perfData)
            .attr('class', 'perf-line')
            .attr('fill', 'none')
            .attr('stroke', '#2196F3')
            .attr('stroke-width', 2)
            .attr('stroke-dasharray', '5,3')
            .attr('d', perfLine);
    }

    function renderResourceOverlay(g, xScale, innerHeight, innerWidth) {
        const resourceData = filteredData.filter(d => d.resources?.peak_cpu_millicores);
        if (resourceData.length === 0) return;

        const maxCpu = d3.max(resourceData, d => d.resources.peak_cpu_millicores) || 1000;
        const cpuYScale = d3.scaleLinear()
            .domain([0, maxCpu * 1.1])
            .range([innerHeight, innerHeight * 0.7]);

        const cpuArea = d3.area()
            .x(d => xScale(d.date))
            .y0(innerHeight)
            .y1(d => cpuYScale(d.resources.peak_cpu_millicores))
            .curve(d3.curveMonotoneX);

        g.append('path')
            .datum(resourceData)
            .attr('class', 'cpu-area')
            .attr('fill', 'rgba(156, 39, 176, 0.2)')
            .attr('stroke', '#9C27B0')
            .attr('stroke-width', 1)
            .attr('d', cpuArea);
    }

    function renderLegend() {
        const legend = document.getElementById('timeline-legend');
        if (!legend) return;

        // Clear existing content safely
        while (legend.firstChild) {
            legend.removeChild(legend.firstChild);
        }

        // Helper to create legend item
        function createLegendItem(className, style, text) {
            const item = document.createElement('div');
            item.className = 'legend-item';

            const indicator = document.createElement('span');
            indicator.className = className;
            Object.assign(indicator.style, style);

            const label = document.createElement('span');
            label.textContent = text;

            item.appendChild(indicator);
            item.appendChild(label);
            return item;
        }

        legend.appendChild(createLegendItem('legend-color', { background: '#4CAF50' }, 'Pass Rate'));

        if (showPerf) {
            legend.appendChild(createLegendItem('legend-line', { borderColor: '#2196F3', borderStyle: 'dashed' }, 'Throughput'));
        }

        if (showResources) {
            legend.appendChild(createLegendItem('legend-area', { background: 'rgba(156, 39, 176, 0.3)' }, 'CPU Usage'));
        }

        if (highlightRegressions) {
            legend.appendChild(createLegendItem('legend-circle', { borderColor: '#F44336' }, 'Regression'));
        }
    }

    // Tooltip
    let tooltip = null;

    function showTooltip(event, d) {
        if (!tooltip) {
            tooltip = document.createElement('div');
            tooltip.className = 'timeline-tooltip';
            Object.assign(tooltip.style, {
                position: 'absolute',
                background: 'var(--bg-secondary, #16213e)',
                color: 'var(--text-primary, #e0e0e0)',
                padding: '10px',
                borderRadius: '4px',
                fontSize: '12px',
                pointerEvents: 'none',
                zIndex: '1000',
                border: '1px solid var(--border, #2a2a4a)',
                boxShadow: '0 2px 10px rgba(0,0,0,0.3)'
            });
            document.body.appendChild(tooltip);
        }

        // Build tooltip content safely using DOM
        while (tooltip.firstChild) {
            tooltip.removeChild(tooltip.firstChild);
        }

        const dateStrong = document.createElement('strong');
        dateStrong.textContent = d.dateStr.split('T')[0];
        tooltip.appendChild(dateStrong);

        tooltip.appendChild(document.createElement('br'));
        tooltip.appendChild(document.createTextNode('Pass Rate: ' + d.passRate.toFixed(1) + '%'));
        tooltip.appendChild(document.createElement('br'));
        tooltip.appendChild(document.createTextNode('Tests: ' + d.passed + '/' + d.totalTests + ' passed'));
        tooltip.appendChild(document.createElement('br'));

        const shaSmall = document.createElement('small');
        shaSmall.textContent = 'SHA: ' + d.commitSha.substring(0, 8);
        tooltip.appendChild(shaSmall);

        if (d.commitMessage) {
            tooltip.appendChild(document.createElement('br'));
            const msgSmall = document.createElement('small');
            const msgText = d.commitMessage.length > 50 ? d.commitMessage.substring(0, 50) + '...' : d.commitMessage;
            msgSmall.textContent = msgText;
            tooltip.appendChild(msgSmall);
        }

        if (d.performance?.metrics?.throughput_per_minute) {
            tooltip.appendChild(document.createElement('br'));
            tooltip.appendChild(document.createTextNode('Throughput: ' + d.performance.metrics.throughput_per_minute.toFixed(1) + '/min'));
        }

        tooltip.style.left = (event.pageX + 10) + 'px';
        tooltip.style.top = (event.pageY - 10) + 'px';
        tooltip.style.opacity = '1';
        tooltip.style.display = 'block';
    }

    function hideTooltip() {
        if (tooltip) {
            tooltip.style.opacity = '0';
            tooltip.style.display = 'none';
        }
    }

    // Detail panel - uses safe DOM methods
    function showRunDetail(d) {
        const panel = document.getElementById('timeline-detail-panel');
        if (!panel) return;

        const detailDate = document.getElementById('detail-date');
        if (detailDate) detailDate.textContent = d.dateStr.split('T')[0];

        // Commit info
        const commitDiv = document.getElementById('detail-commit-info');
        if (commitDiv) {
            while (commitDiv.firstChild) commitDiv.removeChild(commitDiv.firstChild);

            const commitP = document.createElement('p');
            const commitLabel = document.createElement('strong');
            commitLabel.textContent = 'Commit: ';
            commitP.appendChild(commitLabel);
            commitP.appendChild(document.createTextNode(d.commitSha));
            commitDiv.appendChild(commitP);

            const msgP = document.createElement('p');
            const msgLabel = document.createElement('strong');
            msgLabel.textContent = 'Message: ';
            msgP.appendChild(msgLabel);
            msgP.appendChild(document.createTextNode(d.commitMessage || 'N/A'));
            commitDiv.appendChild(msgP);
        }

        // Test summary
        const summaryDiv = document.getElementById('detail-test-summary');
        if (summaryDiv) {
            while (summaryDiv.firstChild) summaryDiv.removeChild(summaryDiv.firstChild);

            function addSummaryLine(label, value) {
                const p = document.createElement('p');
                const strong = document.createElement('strong');
                strong.textContent = label + ': ';
                p.appendChild(strong);
                p.appendChild(document.createTextNode(value));
                summaryDiv.appendChild(p);
            }

            addSummaryLine('Pass Rate', d.passRate.toFixed(1) + '%');
            addSummaryLine('Passed', d.passed + ' / ' + d.totalTests);
            addSummaryLine('Failed', String(d.failed));
        }

        // Perf metrics
        const perfDiv = document.getElementById('detail-perf-metrics');
        if (perfDiv) {
            while (perfDiv.firstChild) perfDiv.removeChild(perfDiv.firstChild);

            if (d.performance?.metrics) {
                const m = d.performance.metrics;
                function addPerfLine(label, value) {
                    const p = document.createElement('p');
                    const strong = document.createElement('strong');
                    strong.textContent = label + ': ';
                    p.appendChild(strong);
                    p.appendChild(document.createTextNode(value));
                    perfDiv.appendChild(p);
                }
                addPerfLine('Scenario', d.performance.scenario || 'N/A');
                addPerfLine('Throughput', (m.throughput_per_minute?.toFixed(1) || 'N/A') + ' runs/min');
                addPerfLine('P50 Latency', (m.p50_latency_seconds?.toFixed(2) || 'N/A') + 's');
                addPerfLine('P95 Latency', (m.p95_latency_seconds?.toFixed(2) || 'N/A') + 's');
            } else {
                const noPerf = document.createElement('p');
                noPerf.className = 'no-perf-data';
                noPerf.textContent = 'No performance data';
                perfDiv.appendChild(noPerf);
            }
        }

        panel.style.display = 'block';
    }

    // Export CSV
    function exportTimelineCSV() {
        if (filteredData.length === 0) {
            alert('No data to export');
            return;
        }

        const headers = ['Date', 'Pass Rate (%)', 'Passed', 'Failed', 'Total', 'Commit SHA', 'Throughput (runs/min)', 'P50 Latency (s)'];
        const rows = filteredData.map(d => [
            d.dateStr.split('T')[0],
            d.passRate.toFixed(1),
            d.passed,
            d.failed,
            d.totalTests,
            d.commitSha,
            d.performance?.metrics?.throughput_per_minute?.toFixed(1) || '',
            d.performance?.metrics?.p50_latency_seconds?.toFixed(2) || ''
        ]);

        const csv = [headers.join(','), ...rows.map(r => r.join(','))].join('\n');

        const blob = new Blob([csv], { type: 'text/csv' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = 'timeline-data.csv';
        a.click();
        URL.revokeObjectURL(url);
    }

    // Expose for external use
    window.timelineModule = {
        loadTimelineData,
        renderTimeline,
        highlightRegressionPoints: function(g, xScale, yScale) {
            highlightRegressionPoints(g, xScale, yScale);
        },
        renderPerfOverlay: function(g, xScale, innerHeight, innerWidth) {
            renderPerfOverlay(g, xScale, innerHeight, innerWidth);
        }
    };

})();
