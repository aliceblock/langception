// Langception Debug Log window (tickets 47/48) — vanilla JS against the
// Tauri global API. Purely observational: the page listens to the
// "loop-event" stream, rendering one card per pipeline pass with
// per-stage durations and the per-block read trace (what OCR read, and
// why each block did or didn't translate) plus the errors that ride the
// stream. History starts when the window opens; no replay of the past.
// Ticket 49: the window keeps a bounded structured mirror of everything
// it showed, and Export… writes it to a picked file — nothing is written
// automatically. Ticket 52: Clear resets all of that in place.
//
// Ticket 70: the window rebuilt to the Debug-Log mock (redesign.html at
// the repo root is this window's own prototype; the mock IS the spec per
// ticket 61's standing rule). What the mock brought beyond the restyle:
// - KPI metric cards (the rolling per-stage summary became the mock's
//   grid), a live pass-counter pill, filter pills (translated / waiting /
//   suppressed, computed per pass), a client-side search over rendered
//   text, an explicit Auto-scroll toggle over the pin-to-bottom follow,
//   the expandable per-pass pipeline diagnostics behind the card header's
//   chevron, toasts instead of the inline status flash, and the export
//   modal (JSON / Plain Text preview, copy to clipboard, download through
//   the real save dialog).
// - The mock's translated-output quote block got a real data source: the
//   forwarder now emits Translated events here too (shell.rs), and they
//   are correlated onto the pass's read trace below.
// What stays prototype-only: the mock's "Simulate Pass" button (its data
// driver, like ticket 61's "Simulate Capture") and its per-pass toast
// (a real pass every cadence tick must not toast); its sample data
// (151 passes, speaker/dialogue read types) has no counterpart in the
// stream — every read renders in the mock's dialogue style. Live
// appends stay incremental; a full re-render happens only for
// filter/search/clear (bounded by MAX_ROWS either way).

const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;
const { save: saveFileDialog } = window.__TAURI__.dialog;
const { getVersion } = window.__TAURI__.app;
const { t } = window.I18n;

const rowsEl = document.getElementById("rows");
const waitingStateEl = document.getElementById("waiting-state");
const emptyStateEl = document.getElementById("empty-state");
const resetFiltersBtn = document.getElementById("reset-filters-btn");
const legendShowingEl = document.getElementById("legend-showing");
const passCountEl = document.getElementById("pass-count");
const passesLabelEl = document.getElementById("passes-label");
const kpiGridEl = document.getElementById("kpi-grid");
const searchInputEl = document.getElementById("search-input");
const clearSearchBtn = document.getElementById("clear-search-btn");
const autoscrollBtn = document.getElementById("autoscroll-btn");
const exportBtn = document.getElementById("export-btn");
const clearBtn = document.getElementById("clear-btn");
const exportModalEl = document.getElementById("export-modal");
const exportPreviewEl = document.getElementById("export-preview");
const fmtJsonBtn = document.getElementById("fmt-json-btn");
const fmtTxtBtn = document.getElementById("fmt-txt-btn");
const copyExportBtn = document.getElementById("copy-export-btn");
const downloadExportBtn = document.getElementById("download-export-btn");
const closeExportModalBtn = document.getElementById("close-export-modal");
const toastContainer = document.getElementById("toast-container");
const countAllEl = document.getElementById("count-all");
const countFilterEls = {
  translated: document.getElementById("count-translated"),
  waiting: document.getElementById("count-waiting"),
  suppressed: document.getElementById("count-suppressed"),
};

// The per-stage durations a timing event can carry (ticket 47). `total`
// rides along everywhere but never competes for "slowest" — it only sums
// the others.
const STAGES = ["capture", "ocr", "translate", "fused"];

// Icons for JS-built markup (inline-SVG Lucide, vendored verbatim —
// CSP forbids the CDN; same path data as ui/main.js's table).
const ICONS = {
  chevronDown:
    '<svg class="chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m6 9 6 6 6-6"/></svg>',
  eye: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M2 12s3-7 10-7 10 7 10 7-3 7-10 7-10-7-10-7Z"/><circle cx="12" cy="12" r="3"/></svg>',
  check: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 6 9 17l-5-5"/></svg>',
  ban: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="10"/><path d="m4.9 4.9 14.2 14.2"/></svg>',
};

// A support tool left open for hours must not grind the DOM down: cards
// beyond this drop oldest-first (both on incremental appends and on
// filter re-renders). The summary keeps every pass since open (or the
// last Clear).
const MAX_ROWS = 500;

// The exportable history (ticket 49): a structured mirror of everything
// the window showed, kept beside the DOM. Bounded like the cards but much
// larger — when the cap trips, the oldest entries drop and the export
// header's covered range says what actually remains.
const MAX_HISTORY = 5000;
const history = [];
// The export header's "session started at" — the window's opening (or,
// after a Clear — ticket 52 — the clear).
let openedAtMs = Date.now();

// Rolling per-stage stats since the window opened — or the last Clear.
// Capture counts every pass; ocr/translate/fused count only the passes
// where they ran; total sums them all. A reported 0ms reading is honest
// (a Glossary-pinned fire) but not charted (ticket 50).
const stats = {
  capture: freshStats(),
  ocr: freshStats(),
  translate: freshStats(),
  fused: freshStats(),
  total: freshStats(),
};
let passCount = 0;

// The card currently accumulating a run of consecutive unchanged ticks
// for its origin (an idle screen collapses into one compact card). Any
// other card breaks the run.
let unchangedRun = null;
// The history entry accumulating the same run (ticket 49) — bumped in
// lockstep with `unchangedRun`.
let openHistoryRun = null;

// View state (ticket 70): the filter pills, the search box, the
// auto-scroll toggle, and the export modal's format.
let currentFilter = "all";
let searchQuery = "";
let autoScroll = true;
let exportFormat = "json";

// Recent translations per origin (ticket 70's quote-block seam): the
// forwarder emits Translated events here, and they arrive BEFORE their
// pass's Timing event (emit_translated runs mid-pass; emit_timing at
// pass end). Buffered keyed by the read's text, consumed by the next
// timing event from that origin, leftovers dropped — a match is exact
// within one pass; anything older is stale.
const pendingShown = new Map();
const MAX_PENDING_ORIGINS = 12;

function freshStats() {
  return { n: 0, sum: 0, min: Infinity, max: 0 };
}

function record(stage, ms) {
  if (ms === 0) return;
  const s = stats[stage];
  s.n += 1;
  s.sum += ms;
  s.min = Math.min(s.min, ms);
  s.max = Math.max(s.max, ms);
}

function originLabel(origin) {
  return origin.kind === "zone" ? origin.name : t("debug.origin.oneshot");
}

// The run key both mirrors of the collapse key on (kind + id — zone names
// are unique per target, this is belt-and-braces).
function originRunKey(event) {
  return `${event.origin.kind}:${event.origin.id ?? ""}`;
}

function clockTime(atMs) {
  return new Date(atMs).toLocaleTimeString();
}

// Ids for history entries: full passes are numbered by their pass count
// (the mock's `pass-147` ids); runs and errors only need uniqueness.
function timingId() {
  return `pass-${passCount}`;
}

// ---- Toasts (the mock's bottom-right feedback; replaces the inline
// ---- export-status flash).

function showToast(message, type = "info", sticky = false) {
  const toast = document.createElement("div");
  toast.className = `toast ${type}`;
  toast.textContent = message;
  toastContainer.append(toast);
  const dismiss = () => {
    toast.style.opacity = "0";
    toast.style.transform = "translateY(6px)";
    setTimeout(() => toast.remove(), 300);
  };
  if (!sticky) setTimeout(dismiss, 2600);
  return dismiss;
}

// The window's only error surface (a refused dialog, a failed write, a
// failed copy): sticky until the next attempt dismisses it.
let activeErrorToast = null;
function showErrorToast(message) {
  if (activeErrorToast !== null) activeErrorToast();
  activeErrorToast = showToast(message, "error", true);
}
function dismissErrorToast() {
  if (activeErrorToast !== null) {
    activeErrorToast();
    activeErrorToast = null;
  }
}

// ---- Scrolling: the whole page scrolls (the mock's model), the sticky
// ---- header rides on top. Follow the stream unless auto-scroll is
// ---- toggled off or the user scrolled up to inspect.

function isPinned() {
  return window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - 32;
}

// PRODUCT.md Accessibility: any motion has a reduced-motion fallback —
// theme.css's neutralizer cannot reach JS scrolling, so "smooth" only
// when the user allows it.
function scrollToBottom(smooth = false) {
  const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  window.scrollTo({
    top: document.documentElement.scrollHeight,
    behavior: smooth && !reduced ? "smooth" : "auto",
  });
}

function trimRows() {
  while (rowsEl.children.length > MAX_ROWS) {
    const dropped = rowsEl.firstElementChild;
    if (dropped === unchangedRun) unchangedRun = null;
    dropped.remove();
  }
}

function appendPinned(card) {
  const pinned = autoScroll && isPinned();
  rowsEl.append(card);
  trimRows();
  if (pinned) scrollToBottom();
}

// Live events append incrementally; a filter or search active means the
// visible set is a view over history — full re-render.
function filterActive() {
  return currentFilter !== "all" || searchQuery.trim() !== "";
}

// ---- Read dispositions (ticket 48) — unchanged from the first build.

// The compact grouping: every translated-* route reports as "translated".
const COUNT_ORDER = [
  "translated",
  "suppressed",
  "waiting",
  "unchanged",
  "excluded",
  "empty",
  "gone",
];

// ReadDisposition is an internally tagged enum (ticket 53): a trace's
// `disposition` is an OBJECT — its own `disposition` key carries the tag.
function dispositionTag(read) {
  return read.disposition.disposition;
}

function countKey(disposition) {
  return disposition.startsWith("translated_") ? "translated" : disposition;
}

// A pass's primary status (ticket 70's filter pills), by the documented
// precedence: translated wins outright, then suppressed over waiting —
// regardless of read order — and blocks_translated > 0 is translated by
// definition. Reads with other dispositions leave the pass under "All"
// only.
function primaryStatusOf(event) {
  let status = null;
  for (const read of event.reads ?? []) {
    const key = countKey(dispositionTag(read));
    if (key === "translated") return "translated";
    if (key === "suppressed") status = "suppressed";
    else if (key === "waiting" && status === null) status = "waiting";
  }
  return event.blocks_translated > 0 ? "translated" : status;
}

// The mock's sub-status pill text — the compact counts, one string.
function readsSummaryText(reads) {
  const counts = new Map();
  for (const read of reads) {
    const key = countKey(dispositionTag(read));
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  const parts = COUNT_ORDER.filter((k) => counts.has(k)).map(
    (k) => `${counts.get(k)} ${t(`debug.count.${k}`)}`
  );
  return reads.length === 1
    ? t("debug.reads_toggle_one", { count: reads.length, parts: parts.join(" · ") })
    : t("debug.reads_toggle_many", { count: reads.length, parts: parts.join(" · ") });
}

// The disposition in plain words — the answer to "why didn't this
// translate?", not the enum's spelling.
function readReason(read) {
  const d = read.disposition;
  switch (dispositionTag(read)) {
    case "translated_engine":
      return t("debug.reason.translated_engine");
    case "translated_glossary":
      return t("debug.reason.translated_glossary");
    case "translated_fused":
      return t("debug.reason.translated_fused");
    case "translated_cached":
      return t("debug.reason.translated_cached");
    case "suppressed":
      return t("debug.reason.suppressed");
    case "waiting": {
      const left = Math.max(0, d.total_ms - d.elapsed_ms);
      return t("debug.reason.waiting", {
        elapsed: d.elapsed_ms,
        total: d.total_ms,
        left,
      });
    }
    case "unchanged":
      return t("debug.reason.unchanged");
    case "excluded":
      return t("debug.reason.excluded");
    case "empty":
      return t("debug.reason.empty");
    case "gone":
      return t("debug.reason.gone", { misses: d.misses });
    default:
      return d.disposition;
  }
}

// The pass's slowest stage (the ◀ mark; `total` excluded — it only sums
// the others). Same rule as the pre-redesign chips and the export.
function slowestStageOf(stages) {
  let slowest = null;
  let slowestMs = -1;
  for (const name of STAGES) {
    const ms = stages[name];
    if (ms !== null && ms > slowestMs) {
      slowestMs = ms;
      slowest = name;
    }
  }
  return slowest;
}

// The slow stage of the session: the largest average among the stages
// that actually ran — the KPI grid's BOTTLENECK card.
function slowestStage() {
  let slowest = null;
  let slowestAvg = -1;
  for (const name of STAGES) {
    const s = stats[name];
    if (s.n > 0 && s.sum / s.n > slowestAvg) {
      slowestAvg = s.sum / s.n;
      slowest = name;
    }
  }
  return slowest;
}

// ---- Header chrome (pass pill, KPI grid, filter counts, legend).

function renderPasses() {
  passCountEl.textContent = String(passCount);
  passesLabelEl.textContent =
    passCount === 1 ? t("debug.passes_unit_one") : t("debug.passes_unit_many");
}

function renderKPIs() {
  const slowest = slowestStage();
  kpiGridEl.replaceChildren();
  for (const name of [...STAGES, "total"]) {
    const s = stats[name];
    if (s.n === 0) continue;
    const card = document.createElement("div");
    card.className = `kpi-card kpi-${name}` + (name === slowest ? " slow" : "");
    const head = document.createElement("div");
    head.className = "kpi-head";
    const nameEl = document.createElement("span");
    nameEl.className = "kpi-name";
    const dot = document.createElement("span");
    dot.className = "kpi-dot";
    nameEl.append(dot, document.createTextNode(t(`debug.stage.${name}`)));
    if (name === slowest) {
      const badge = document.createElement("span");
      badge.className = "kpi-badge";
      badge.textContent = t("debug.kpi.bottleneck");
      nameEl.append(badge);
    }
    const count = document.createElement("span");
    count.className = "kpi-count";
    count.textContent = `×${s.n}`;
    head.append(nameEl, count);
    const values = document.createElement("div");
    values.className = "kpi-values";
    const avg = document.createElement("span");
    avg.className = "kpi-avg";
    avg.textContent = t("debug.kpi.avg", { avg: Math.round(s.sum / s.n) });
    const range = document.createElement("span");
    range.className = "kpi-range";
    range.textContent = t("debug.kpi.range", { min: s.min, max: s.max });
    values.append(avg, range);
    card.append(head, values);
    kpiGridEl.append(card);
  }
}

function renderFilterCounts() {
  countAllEl.textContent = String(history.length);
  for (const f of ["translated", "waiting", "suppressed"]) {
    countFilterEls[f].textContent = String(
      history.filter((e) => e.kind === "timing" && e.primaryStatus === f).length
    );
  }
}

function renderLegend(visibleCount) {
  // The mock bolds the count (mono like every number in the legend), so
  // the rendered string is split around it — the same split-the-label
  // move the Settings slider's live value uses.
  legendShowingEl.replaceChildren();
  const text =
    visibleCount === 1
      ? t("debug.legend.showing_one", { count: visibleCount })
      : t("debug.legend.showing_many", { count: visibleCount });
  const num = String(visibleCount);
  const at = text.indexOf(num);
  if (at >= 0) {
    const strong = document.createElement("strong");
    strong.textContent = num;
    legendShowingEl.append(
      document.createTextNode(text.slice(0, at)),
      strong,
      document.createTextNode(text.slice(at + num.length)),
    );
  } else {
    legendShowingEl.textContent = text;
  }
}

function renderLegendFromDom() {
  renderLegend(rowsEl.children.length);
}

// The session controls (Export…, Clear) stay hidden while the window is
// empty — nothing to export, nothing to clear — and appear with the first
// RENDERED event (tickets 49/52; the invariant "buttons visible ⟺ a card
// and a history entry exist"). Translated events only feed the
// quote-block buffer and never render alone — they don't count here.
// The same moment ends the waiting state (the mock's empty card).
function showSessionControls() {
  exportBtn.hidden = false;
  clearBtn.hidden = false;
  waitingStateEl.hidden = true;
}

// ---- Card builders (the mock's log cards).

function spanEl(className, text) {
  const el = document.createElement("span");
  if (className) el.className = className;
  if (text !== undefined) el.textContent = text;
  return el;
}

function stagePill(name, ms, isSlow) {
  const pill = document.createElement("span");
  pill.className = `stage-pill sc-${name}` + (isSlow ? " slow" : "");
  const label = document.createElement("span");
  label.className = "stage-name";
  label.textContent = t(`debug.stage.${name}`);
  pill.append(label, document.createTextNode(`${ms}ms`));
  if (isSlow) pill.append(spanEl("slow-mark", "◀"));
  return pill;
}

function readBadge(read) {
  const key = countKey(dispositionTag(read));
  if (key === "translated") {
    const b = document.createElement("span");
    b.className = "reason-badge reason-translated";
    b.insertAdjacentHTML("afterbegin", ICONS.check);
    b.append(document.createTextNode(readReason(read)));
    return b;
  }
  if (key === "waiting") {
    const b = document.createElement("span");
    b.className = "reason-badge reason-waiting";
    const dot = document.createElement("span");
    dot.className = "waiting-dot";
    b.append(dot, document.createTextNode(readReason(read)));
    return b;
  }
  if (key === "suppressed") {
    const b = document.createElement("span");
    b.className = "reason-badge reason-suppressed";
    b.insertAdjacentHTML("afterbegin", ICONS.ban);
    b.append(document.createTextNode(readReason(read)));
    return b;
  }
  // unchanged / excluded / empty: plain dim mono, like the mock's
  // unchanged detail; gone stays the alarming rose.
  const b = document.createElement("span");
  b.className = "reason" + (key === "gone" ? " gone" : "");
  b.textContent = readReason(read);
  return b;
}

function readLine(read) {
  const line = document.createElement("div");
  line.className = "read-line";
  const words = document.createElement("div");
  words.className = "read-words";
  words.textContent = read.text === "" ? t("debug.nothing_after_trim") : `“${read.text}”`;
  line.append(words);
  const foot = document.createElement("div");
  foot.className = "read-foot";
  foot.append(readBadge(read));
  line.append(foot);
  // The mock's emerald quote block — the correlated translation text.
  if (read.shown !== undefined) {
    const shown = document.createElement("div");
    shown.className = "read-shown";
    shown.textContent = read.shown;
    line.append(shown);
  }
  return line;
}

function diagnosticsFor(entry) {
  const box = document.createElement("div");
  box.className = "diagnostics";
  const headRow = document.createElement("div");
  headRow.className = "diagnostics-head";
  headRow.append(
    spanEl("", t("debug.diagnostics.title")),
    spanEl("", t("debug.diagnostics.overhead", {
      ms: Math.max(0, entry.totalMs - STAGES.reduce((acc, n) => acc + (entry.stages[n] ?? 0), 0)),
    })),
  );
  const grid = document.createElement("div");
  grid.className = "diagnostics-grid";
  const totalForPct = Math.max(entry.totalMs, 1);
  for (const name of STAGES) {
    const ms = entry.stages[name];
    // The mock's three cells (capture / ocr / translate, skipped ones
    // labeled); the fused cell appears only when the fused path ran.
    if (name === "fused" && ms === null) continue;
    const cell = document.createElement("div");
    const label = document.createElement("span");
    label.className = `diag-stage-label sc-${name}`;
    label.textContent = t(`debug.stage.${name}`);
    const value = document.createElement("span");
    value.textContent =
      ms !== null
        ? `${ms}ms (${Math.round((ms / totalForPct) * 100)}%)`
        : t("debug.diagnostics.skipped");
    cell.append(label, value);
    grid.append(cell);
  }
  box.append(headRow, grid);
  return box;
}

function buildTimingCard(entry) {
  const card = document.createElement("article");
  card.className =
    "card" +
    (entry.primaryStatus ? ` status-${entry.primaryStatus}` : "") +
    (entry.expanded ? " expanded" : "");
  card.dataset.id = entry.id;

  const head = document.createElement("div");
  head.className = "card-head";
  const left = document.createElement("div");
  left.className = "card-head-left";
  left.append(
    spanEl("card-time", clockTime(entry.atMs)),
    spanEl("origin-badge", entry.origin),
  );
  const pills = document.createElement("div");
  pills.className = "stage-pills";
  for (const name of STAGES) {
    const ms = entry.stages[name];
    if (ms !== null) pills.append(stagePill(name, ms, name === entry.slowestStage));
  }
  if (entry.readSkipped) pills.append(spanEl("no-read-marker", t("debug.no_read")));
  left.append(
    pills,
    spanEl(
      "blocks-count" + (entry.blocks > 0 ? " fired" : ""),
      entry.blocks === 1
        ? t("debug.blocks_one", { count: entry.blocks })
        : t("debug.blocks_many", { count: entry.blocks }),
    ),
  );
  const right = document.createElement("div");
  right.className = "card-head-right";
  const total = document.createElement("span");
  total.className = "total-badge" + (entry.totalMs >= 500 ? " slow" : "");
  total.title = t("debug.total.title");
  total.append(spanEl("total-word", t("debug.stage.total")), document.createTextNode(`${entry.totalMs}ms`));
  right.append(total);
  right.insertAdjacentHTML("beforeend", ICONS.chevronDown);
  head.append(left, right);
  card.append(head);

  // The proportional latency bar (capture vs ocr vs translate vs fused).
  const bar = document.createElement("div");
  bar.className = "latency-bar";
  const totalForBar = Math.max(entry.totalMs, 1);
  for (const name of STAGES) {
    const ms = entry.stages[name];
    if (ms === null) continue;
    const seg = document.createElement("div");
    seg.className = `seg-${name}`;
    seg.style.width = `${Math.max(4, Math.round((ms / totalForBar) * 100))}%`;
    bar.append(seg);
  }
  card.append(bar);

  const sub = document.createElement("div");
  sub.className = "substatus";
  const summary = document.createElement("span");
  summary.className = "reads-summary";
  summary.insertAdjacentHTML("afterbegin", ICONS.eye);
  summary.append(document.createTextNode(entry.readsSummary));
  sub.append(summary, spanEl("pass-id", entry.id));
  card.append(sub);

  if (entry.reads.length) {
    const box = document.createElement("div");
    box.className = "reads-box";
    const inner = document.createElement("div");
    inner.className = "reads-inner";
    for (const read of entry.reads) inner.append(readLine(read));
    box.append(inner);
    card.append(box);
  }

  if (entry.expanded) card.append(diagnosticsFor(entry));
  return card;
}

function buildUnchangedCard(entry) {
  const card = document.createElement("article");
  card.className = "card unchanged";
  card.dataset.id = entry.id;
  card.dataset.origin = entry.runKey;
  const line = document.createElement("div");
  line.className = "unchanged-line";
  line.append(
    spanEl("card-time", clockTime(entry.atMs)),
    spanEl("origin-badge", entry.origin),
  );
  const label = spanEl("", t("debug.unchanged.label"));
  label.append(spanEl("count", String(entry.count)));
  line.append(label);
  const capture = spanEl("", t("debug.unchanged.capture").trimStart());
  capture.append(spanEl("capture-ms", `${entry.captureMs}ms`));
  line.append(capture);
  card.append(line);
  return card;
}

function buildErrorCard(entry) {
  const card = document.createElement("article");
  card.className = "card error";
  const line = document.createElement("div");
  line.className = "error-line";
  line.append(
    spanEl("card-time", clockTime(entry.atMs)),
    spanEl("error-stage", entry.stage),
    spanEl("error-message", entry.message),
  );
  card.append(line);
  return card;
}

function cardFor(entry) {
  if (entry.kind === "timing") return buildTimingCard(entry);
  if (entry.kind === "unchanged") return buildUnchangedCard(entry);
  return buildErrorCard(entry);
}

// ---- The filter/search view (ticket 70): a pure function over history;
// ---- full re-render on every view change and on live events while a
// ---- view is active.

function entryMatches(entry) {
  if (currentFilter !== "all") {
    if (entry.kind !== "timing" || entry.primaryStatus !== currentFilter) return false;
  }
  const q = searchQuery.trim().toLowerCase();
  if (q === "") return true;
  if (entry.kind === "error") {
    return entry.stage.toLowerCase().includes(q) || entry.message.toLowerCase().includes(q);
  }
  if (
    entry.origin.toLowerCase().includes(q) ||
    entry.id.toLowerCase().includes(q) ||
    clockTime(entry.atMs).toLowerCase().includes(q)
  ) {
    return true;
  }
  if (entry.kind === "unchanged") return entry.kind.includes(q);
  if (entry.readsSummary.toLowerCase().includes(q)) return true;
  return entry.reads.some(
    (read) =>
      read.text.toLowerCase().includes(q) ||
      readReason(read).toLowerCase().includes(q) ||
      (read.shown !== undefined && read.shown.toLowerCase().includes(q))
  );
}

// A view re-render: view ACTIONS (filter click, typing, reset) land the
// user where they are — the mock never scrolls on those — while the live
// path passes follow=true, which honors the same contract as appends
// (only when auto-scroll is on AND the user is still at the bottom).
function renderStream(follow = false) {
  const filtered = history.filter(entryMatches);
  renderLegend(filtered.length);
  renderFilterCounts();
  waitingStateEl.hidden = history.length !== 0;
  emptyStateEl.hidden = history.length === 0 || filtered.length !== 0;
  rowsEl.replaceChildren();
  unchangedRun = null;
  for (const entry of filtered.slice(-MAX_ROWS)) {
    const card = cardFor(entry);
    rowsEl.append(card);
    if (entry === openHistoryRun) unchangedRun = card;
  }
  if (follow && autoScroll && isPinned()) scrollToBottom();
}

function appendLive(entry) {
  renderFilterCounts();
  if (filterActive()) {
    renderStream(true);
    return;
  }
  const card = cardFor(entry);
  card.classList.add("log-enter");
  appendPinned(card);
  renderLegendFromDom();
}

// ---- Event handlers (the stream's three shapes).

function handleTiming(event) {
  showSessionControls();
  passCount += 1;
  record("capture", event.capture_ms);
  for (const name of STAGES.slice(1)) {
    const ms = event[`${name}_ms`];
    if (ms !== null) record(name, ms);
  }
  record("total", event.total_ms);
  renderPasses();
  renderKPIs();

  if (event.read_skipped && event.blocks_translated === 0) {
    // A tick that only captured and checked (the fast path): collapse
    // consecutive runs so an idle screen costs one compact card. A pass
    // that DIED before reading is not a skip — it keeps its full card
    // (ticket 48's explicit read_skipped flag). The quote-block buffer
    // is consumed/cleared here too: nothing fired on this pass, so any
    // pending translation for this origin is stale and must not attach
    // to some later pass that happens to repeat the text.
    attachShownTranslations(event);
    const runKey = originRunKey(event);
    if (unchangedRun !== null && unchangedRun.dataset.origin === runKey) {
      const entry = openHistoryRun;
      entry.count += 1;
      entry.captureMs = event.capture_ms;
      entry.atMs = event.at_ms;
      unchangedRun.querySelector(".count").textContent = String(entry.count);
      unchangedRun.querySelector(".capture-ms").textContent = `${event.capture_ms}ms`;
      unchangedRun.querySelector(".card-time").textContent = clockTime(event.at_ms);
      if (autoScroll && isPinned()) scrollToBottom();
      return;
    }
    const entry = {
      kind: "unchanged",
      id: `run-${passCount}`,
      runKey,
      origin: originLabel(event.origin),
      count: 1,
      captureMs: event.capture_ms,
      atMs: event.at_ms,
    };
    openHistoryRun = entry;
    pushHistory(entry);
    if (filterActive()) {
      renderStream(true);
      return;
    }
    const card = buildUnchangedCard(entry);
    card.classList.add("log-enter");
    appendPinned(card);
    unchangedRun = card;
    renderLegendFromDom();
    return;
  }

  attachShownTranslations(event);
  const stages = {
    capture: event.capture_ms,
    ocr: event.ocr_ms,
    translate: event.translate_ms,
    fused: event.fused_ms,
  };
  const reads = event.reads ?? [];
  const entry = {
    kind: "timing",
    id: timingId(),
    atMs: event.at_ms,
    origin: originLabel(event.origin),
    isZone: event.origin.kind === "zone",
    readSkipped: event.read_skipped,
    stages,
    slowestStage: slowestStageOf(stages),
    totalMs: event.total_ms,
    blocks: event.blocks_translated,
    primaryStatus: primaryStatusOf(event),
    reads,
    readsSummary: readsSummaryText(reads),
    expanded: false,
  };
  openHistoryRun = null;
  pushHistory(entry);
  appendLive(entry);
}

function handleError(event) {
  showSessionControls();
  const entry = {
    kind: "error",
    id: `err-${passCount}`,
    atMs: event.at_ms,
    stage: event.stage,
    message: event.message,
  };
  openHistoryRun = null;
  pushHistory(entry);
  appendLive(entry);
}

// Ticket 70's quote-block seam: buffer this Translated event's text for
// the origin's next Timing event.
function handleTranslated(event) {
  const text = event.block && event.block.text;
  if (typeof text !== "string" || typeof event.translation !== "string") return;
  const key = originRunKey(event);
  let buf = pendingShown.get(key);
  if (buf === undefined) {
    buf = new Map();
    pendingShown.set(key, buf);
  }
  buf.set(text, event.translation);
  // Bound the buffer: drop the oldest origin beyond the cap.
  while (pendingShown.size > MAX_PENDING_ORIGINS) {
    const oldest = pendingShown.keys().next().value;
    if (oldest === key) break;
    pendingShown.delete(oldest);
  }
}

// Consume the buffer into this pass's translated reads (exact text match,
// same pass), then clear whatever is left — those belong to a pass whose
// timing already passed, so they can never match later.
function attachShownTranslations(event) {
  const key = originRunKey(event);
  const buf = pendingShown.get(key);
  if (buf === undefined) return;
  for (const read of event.reads ?? []) {
    if (countKey(dispositionTag(read)) !== "translated") continue;
    const shown = buf.get(read.text);
    if (shown !== undefined) {
      read.shown = shown;
      buf.delete(read.text);
    }
  }
  pendingShown.delete(key);
}

// ---- The exportable history (ticket 49) — one entry per thing the
// ---- window showed, pushed in stream order. The exports are pure
// ---- functions of this array.

function pushHistory(entry) {
  history.push(entry);
  while (history.length > MAX_HISTORY) {
    const dropped = history.shift();
    if (dropped === openHistoryRun) openHistoryRun = null;
  }
}

// ---- Export (ticket 49): the text format unchanged; the mock's modal
// ---- adds a JSON format, a copy button, and the same save-dialog
// ---- download behind the format's extension.

function pad2(n) {
  return String(n).padStart(2, "0");
}

function stamp(ms) {
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())} ` +
    `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`;
}

// The export's origin label: the window's bare zone name gains a Zone
// prefix so it reads as a name in prose (one-shots already say One-shot).
function exportOrigin(entry) {
  return entry.isZone ? t("debug.export.zone_prefix", { name: entry.origin }) : entry.origin;
}

// One block head per full pass: the stages that ran in STAGES order with
// the pass's slowest marked (like the card's pills), then total, blocks,
// and the fast-path marker.
function passLine(entry) {
  const parts = [exportOrigin(entry)];
  for (const name of STAGES) {
    const ms = entry.stages[name];
    if (ms === null) continue;
    parts.push(
      t("debug.export.stage_part", { name: t(`debug.stage.${name}`), ms }) +
        (name === entry.slowestStage ? " ◂" : ""),
    );
  }
  parts.push(t("debug.export.pass_total", { ms: entry.totalMs }));
  parts.push(
    entry.blocks === 1
      ? t("debug.blocks_one", { count: entry.blocks })
      : t("debug.blocks_many", { count: entry.blocks }),
  );
  if (entry.readSkipped) parts.push(t("debug.no_read"));
  return `--- ${clockTime(entry.atMs)} · ${parts.join(" · ")}`;
}

// The per-block read trace: the same words and plain-words reasons the
// card shows; a correlated translation gets its own continuation line, so
// a paste reads like the window (quote block included).
function readsLines(reads) {
  const prefix = t("debug.export.reads_prefix", { count: reads.length });
  const lines = [];
  for (const [i, read] of reads.entries()) {
    const words = read.text === "" ? t("debug.nothing_after_trim") : `“${read.text}”`;
    const indent = i === 0 ? prefix : " ".repeat(prefix.length);
    lines.push(`${indent}${words} → ${readReason(read)}`);
    if (read.shown !== undefined) {
      lines.push(`${" ".repeat(prefix.length)}  ↳ “${read.shown}”`);
    }
  }
  return lines;
}

function summaryLines() {
  const slowest = slowestStage();
  const lines = ["", t("debug.export.per_stage_summary")];
  for (const name of [...STAGES, "total"]) {
    const s = stats[name];
    if (s.n === 0) continue;
    const mark = name === slowest ? t("debug.export.slowest_mark") : "";
    lines.push(
      t("debug.export.stage_line", {
        name: t(`debug.stage.${name}`),
        avg: Math.round(s.sum / s.n),
        min: s.min,
        max: s.max,
        n: s.n,
        mark,
      }),
    );
  }
  return lines;
}

function buildExportText(version) {
  const coveredEvents =
    history.length === 1
      ? t("debug.export.covered_range_one", {
          from: clockTime(history[0].atMs),
          to: clockTime(history[history.length - 1].atMs),
          count: history.length,
        })
      : t("debug.export.covered_range_many", {
          from: clockTime(history[0].atMs),
          to: clockTime(history[history.length - 1].atMs),
          count: history.length,
        });
  const lines = [
    t("debug.export.title"),
    t("debug.export.app_version", { version }),
    t("debug.export.exported_at", { time: stamp(Date.now()) }),
    t("debug.export.session_started", { time: stamp(openedAtMs) }),
    t("debug.export.passes", { count: passCount }),
    coveredEvents,
  ];
  lines.push(...summaryLines());
  lines.push("");
  for (const entry of history) {
    if (entry.kind === "timing") {
      lines.push(passLine(entry));
      if (entry.reads.length) lines.push(...readsLines(entry.reads));
    } else if (entry.kind === "unchanged") {
      lines.push(
        `--- ${clockTime(entry.atMs)} · ${exportOrigin(entry)} · ` +
          `${t("debug.unchanged.label")}${entry.count} · ` +
          `${t("debug.unchanged.capture").trimStart()}${entry.captureMs}ms`
      );
    } else {
      lines.push(
        `--- ${clockTime(entry.atMs)} · ${t("debug.export.error_word")} · ${entry.stage} · ${entry.message}`
      );
    }
  }
  return lines.join("\n") + "\n";
}

function statsJson(s) {
  return s.n === 0 ? { count: 0 } : { count: s.n, avg: Math.round(s.sum / s.n), min: s.min, max: s.max };
}

function historyJson() {
  return history.map((entry) => {
    if (entry.kind === "timing") {
      return {
        kind: entry.kind,
        id: entry.id,
        at: stamp(entry.atMs),
        origin: exportOrigin(entry),
        readSkipped: entry.readSkipped,
        stages: entry.stages,
        totalMs: entry.totalMs,
        blocks: entry.blocks,
        status: entry.primaryStatus,
        reads: entry.reads.map((read) => {
          const out = { text: read.text, disposition: read.disposition, reason: readReason(read) };
          if (read.shown !== undefined) out.shown = read.shown;
          return out;
        }),
      };
    }
    if (entry.kind === "unchanged") {
      return {
        kind: entry.kind,
        at: stamp(entry.atMs),
        origin: exportOrigin(entry),
        count: entry.count,
        captureMs: entry.captureMs,
      };
    }
    return { kind: entry.kind, at: stamp(entry.atMs), stage: entry.stage, message: entry.message };
  });
}

function buildExportJson() {
  return JSON.stringify(
    {
      exportedAt: new Date().toISOString(),
      summary: {
        passes: passCount,
        capture: statsJson(stats.capture),
        ocr: statsJson(stats.ocr),
        translate: statsJson(stats.translate),
        fused: statsJson(stats.fused),
        total: statsJson(stats.total),
      },
      entries: historyJson(),
    },
    null,
    2,
  );
}

// ---- The export modal (ticket 70).

let cachedVersion = null;
async function ensureVersion() {
  if (cachedVersion === null) {
    try {
      cachedVersion = await getVersion();
    } catch {
      cachedVersion = "?";
    }
  }
  return cachedVersion;
}

function generateExportContent() {
  return exportFormat === "json" ? buildExportJson() : buildExportText(cachedVersion ?? "?");
}

async function openExportModal() {
  dismissErrorToast();
  await ensureVersion();
  exportPreviewEl.value = generateExportContent();
  exportModalEl.hidden = false;
}

function closeExportModal() {
  exportModalEl.hidden = true;
}

function setExportFormat(format) {
  exportFormat = format;
  fmtJsonBtn.classList.toggle("active", format === "json");
  fmtTxtBtn.classList.toggle("active", format === "txt");
  exportPreviewEl.value = generateExportContent();
}

function copyExport() {
  exportPreviewEl.select();
  let copied = false;
  try {
    copied = document.execCommand("copy");
  } catch {
    copied = false;
  }
  if (copied) showToast(t("debug.export.copied"), "success");
  else showErrorToast(t("debug.export.copy_failed"));
}

function defaultExportName(ext) {
  const d = new Date();
  return `langception-debug-${d.getFullYear()}${pad2(d.getMonth() + 1)}${pad2(d.getDate())}` +
    `-${pad2(d.getHours())}${pad2(d.getMinutes())}${pad2(d.getSeconds())}.${ext}`;
}

// Download = the real file write (ticket 49's save dialog +
// export_debug_log command), now in the picked format.
async function downloadExport() {
  dismissErrorToast();
  try {
    const isJson = exportFormat === "json";
    const path = await saveFileDialog({
      title: t("debug.export.dialog_title"),
      defaultPath: defaultExportName(isJson ? "json" : "txt"),
      filters: [
        isJson
          ? { name: t("debug.export.filter_json"), extensions: ["json"] }
          : { name: t("debug.export.filter_text"), extensions: ["txt"] },
      ],
    });
    if (!path) return;
    await invoke("export_debug_log", { path, contents: exportPreviewEl.value });
    showToast(t("debug.export.saved"), "success");
  } catch (e) {
    showErrorToast(String(e));
  }
}

exportBtn.addEventListener("click", openExportModal);
closeExportModalBtn.addEventListener("click", closeExportModal);
exportModalEl.addEventListener("click", (e) => {
  if (e.target === exportModalEl) closeExportModal();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !exportModalEl.hidden) closeExportModal();
});
fmtJsonBtn.addEventListener("click", () => setExportFormat("json"));
fmtTxtBtn.addEventListener("click", () => setExportFormat("txt"));
copyExportBtn.addEventListener("click", copyExport);
downloadExportBtn.addEventListener("click", downloadExport);

// ---- Ticket 52: the Clear button — one click resets every piece of
// ---- window state the way reopening the window would.

function clearAll() {
  // The view resets with everything else (like reopening the window):
  // a stale filter/search must not keep incoming events invisible.
  currentFilter = "all";
  searchQuery = "";
  searchInputEl.value = "";
  clearSearchBtn.hidden = true;
  for (const b of document.querySelectorAll(".filter-pill")) {
    b.classList.toggle("active", b.dataset.filter === "all");
  }
  rowsEl.replaceChildren();
  unchangedRun = null; // a run must not continue across a clear
  openHistoryRun = null;
  history.length = 0;
  pendingShown.clear();
  openedAtMs = Date.now();
  passCount = 0;
  for (const name of [...STAGES, "total"]) stats[name] = freshStats();
  renderPasses();
  renderKPIs();
  renderFilterCounts();
  renderLegendFromDom();
  waitingStateEl.hidden = false;
  emptyStateEl.hidden = true;
  exportBtn.hidden = true;
  clearBtn.hidden = true;
  showToast(t("debug.clear.cleared"), "warning");
}
clearBtn.addEventListener("click", clearAll);

// ---- View controls: filter pills, search, reset, auto-scroll.

for (const btn of document.querySelectorAll(".filter-pill")) {
  btn.addEventListener("click", () => {
    for (const b of document.querySelectorAll(".filter-pill")) b.classList.remove("active");
    btn.classList.add("active");
    currentFilter = btn.dataset.filter;
    renderStream();
  });
}

searchInputEl.addEventListener("input", () => {
  searchQuery = searchInputEl.value;
  clearSearchBtn.hidden = searchQuery === "";
  renderStream();
});

// Clearing the search box and resetting the whole view share one recipe
// (the ✕ button resets the query only; Reset Filters also restores the
// All pill) — both end in a re-render.
function clearSearchOnly() {
  searchQuery = "";
  searchInputEl.value = "";
  clearSearchBtn.hidden = true;
  renderStream();
}

clearSearchBtn.addEventListener("click", clearSearchOnly);

resetFiltersBtn.addEventListener("click", () => {
  clearSearchOnly();
  currentFilter = "all";
  for (const b of document.querySelectorAll(".filter-pill")) {
    b.classList.toggle("active", b.dataset.filter === "all");
  }
  renderStream();
});

autoscrollBtn.addEventListener("click", () => {
  autoScroll = !autoScroll;
  autoscrollBtn.setAttribute("aria-pressed", String(autoScroll));
  if (autoScroll) {
    scrollToBottom(true);
    showToast(t("debug.autoscroll.enabled"), "success");
  } else {
    showToast(t("debug.autoscroll.paused"), "info");
  }
});

// Expand a card's pipeline diagnostics (the header row's chevron). The
// expanded flag lives on the history entry, so filters re-render keep it.
rowsEl.addEventListener("click", (e) => {
  const head = e.target.closest(".card-head");
  if (head === null) return;
  const card = head.closest(".card");
  const entry = history.find((en) => en.kind === "timing" && en.id === card.dataset.id);
  if (entry === undefined) return;
  entry.expanded = !entry.expanded;
  card.replaceWith(buildTimingCard(entry));
});

// ---- Boot (ticket 62's seam; ticket 61's language follow).

window.addEventListener("DOMContentLoaded", async () => {
  try {
    await window.I18n.init(await invoke("get_ui_language"));
  } catch {
    await window.I18n.init("en"); // offline of the backend: English
  }
  renderPasses();
  renderKPIs();
  renderFilterCounts();
  renderLegendFromDom();

  await listen("loop-event", (event) => {
    const payload = event.payload;
    if (payload.type === "timing") {
      handleTiming(payload);
    } else if (payload.type === "error") {
      handleError(payload);
    } else if (payload.type === "translated") {
      handleTranslated(payload);
    }
    // BlocksRemoved stays with the overlays; the debug view is timings,
    // errors, and the translated texts feeding the quote blocks.
  });

  // Ticket 61: a UI-Language saved in Settings reaches this window on the
  // same engine-state broadcast — re-resolve the dictionaries so the
  // static labels and the dynamic chrome (pass pill, KPI cards, counts,
  // legend) follow immediately. Cards already rendered keep their
  // original language, frozen artifacts.
  await listen("engine-state", async (event) => {
    const next = event.payload && event.payload.uiLanguage;
    if (next && next !== window.I18n.language()) {
      await window.I18n.init(next);
      renderPasses();
      renderKPIs();
      renderFilterCounts();
      renderLegendFromDom();
    }
  });
});
