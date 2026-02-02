// filters.js -- Reactive filter state management + URL hash sync

const state = {
  category: null,
  status: null,
  component: null,
  dateFrom: null,
  dateTo: null,
  search: "",
  selectedRuns: [],
};

let _onChange = null;

/**
 * Wire DOM event listeners on all filter inputs.
 * On any change, update state and call onChangeCallback(state).
 */
export function initFilters(onChangeCallback) {
  _onChange = onChangeCallback;

  const bind = (id, key, event = "input") => {
    const el = document.getElementById(id);
    if (!el) return;
    el.addEventListener(event, () => {
      state[key] = el.value || null;
      syncToUrl(state);
      if (_onChange) _onChange(state);
    });
  };

  bind("filter-category", "category", "change");
  bind("filter-status", "status", "change");
  bind("filter-component", "component");
  bind("filter-date-from", "dateFrom", "change");
  bind("filter-date-to", "dateTo", "change");
  bind("filter-search", "search");

  // Listen for hash changes (back/forward navigation)
  window.addEventListener("hashchange", () => {
    loadFromUrl();
    if (_onChange) _onChange(state);
  });
}

/**
 * Merge patch into state, sync to URL, call registered callback.
 */
export function updateState(patch) {
  Object.assign(state, patch);
  syncToUrl(state);
  if (_onChange) _onChange(state);
}

/**
 * Encode non-null/non-empty state values into URL hash using URLSearchParams.
 */
export function syncToUrl(s) {
  const params = new URLSearchParams();
  if (s.category) params.set("category", s.category);
  if (s.status) params.set("status", s.status);
  if (s.component) params.set("component", s.component);
  if (s.dateFrom) params.set("dateFrom", s.dateFrom);
  if (s.dateTo) params.set("dateTo", s.dateTo);
  if (s.search) params.set("search", s.search);
  if (s.selectedRuns && s.selectedRuns.length > 0) {
    params.set("runs", s.selectedRuns.join(","));
  }
  const hash = params.toString();
  history.replaceState(null, "", hash ? "#" + hash : window.location.pathname);
}

/**
 * Parse URL hash and restore state. Return the restored state.
 */
export function loadFromUrl() {
  const hash = window.location.hash.slice(1);
  if (!hash) return state;

  const params = new URLSearchParams(hash);

  const setIfPresent = (key, paramName) => {
    const v = params.get(paramName || key);
    state[key] = v || null;
  };

  setIfPresent("category");
  setIfPresent("status");
  setIfPresent("component");
  setIfPresent("dateFrom");
  setIfPresent("dateTo");
  state.search = params.get("search") || "";

  const runsParam = params.get("runs");
  state.selectedRuns = runsParam ? runsParam.split(",") : [];

  // Sync DOM elements with restored state
  _syncDom();

  return state;
}

/**
 * Filter runs by dateFrom/dateTo.
 */
export function filterRuns(runs, s) {
  if (!runs) return [];
  return runs.filter((r) => {
    const d = new Date(r.date);
    if (s.dateFrom && d < new Date(s.dateFrom)) return false;
    if (s.dateTo && d > new Date(s.dateTo + "T23:59:59")) return false;
    return true;
  });
}

/**
 * Filter tests by: category, status, component (spec), search (scenario).
 */
export function filterTests(tests, s) {
  if (!tests) return [];
  return tests.filter((t) => {
    if (s.category && t.category !== s.category) return false;
    if (s.status) {
      const ts = (t.status || "").toLowerCase();
      if (s.status.toLowerCase() === "passed" && ts !== "pass" && ts !== "passed") return false;
      if (s.status.toLowerCase() === "failed" && ts !== "fail" && ts !== "failed") return false;
    }
    if (s.component && !(t.spec || "").toLowerCase().includes(s.component.toLowerCase())) return false;
    if (s.search && !(t.scenario || t.name || "").toLowerCase().includes(s.search.toLowerCase())) return false;
    return true;
  });
}

/**
 * Return current state.
 */
export function getState() {
  return state;
}

/**
 * Sync DOM filter elements with current state values.
 */
function _syncDom() {
  const setVal = (id, val) => {
    const el = document.getElementById(id);
    if (el) el.value = val || "";
  };
  setVal("filter-category", state.category);
  setVal("filter-status", state.status);
  setVal("filter-component", state.component);
  setVal("filter-date-from", state.dateFrom);
  setVal("filter-date-to", state.dateTo);
  setVal("filter-search", state.search);
}
