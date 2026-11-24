// Langception Debug Log window (tickets 47/48) — vanilla JS against the
// Tauri global API. Purely observational: the page only listens to the
// same "loop-event" stream the Results Window uses, rendering one row per
// pipeline pass with per-stage durations and the per-block read trace
// (what OCR read, and why each block did or didn't translate) plus the
// errors that ride the stream. History starts when the window opens; no
// replay of the past. Ticket 49: the window also keeps a bounded
// structured mirror of everything it showed, and an Export… button writes
// it to a picked file as readable text — nothing is written
// automatically, the manual export is the only file this window makes.
// Ticket 52: Clear resets all of that in place — a fresh session without
// reopening the window; the loop itself is untouched.

const { listen } = window.__TAURI__.event;
const { invoke } = window.__TAURI__.core;
const { save: saveFileDialog } = window.__TAURI__.dialog;
const { getVersion } = window.__TAURI__.app;
const { t } = window.I18n;

const summaryEl = document.getElementById("summary");
const rowsEl = document.getElementById("rows");
const exportBtn = document.getElementById("export-btn");
const clearBtn = document.getElementById("clear-btn");
const exportStatusEl = document.getElementById("export-status");

// The empty state's wording — the HTML ships the same text (data-i18n);
// Clear puts it back verbatim through the same key.
const EMPTY_SUMMARY_KEY = "debug.empty_summary";

// The per-stage durations a timing event can carry (ticket 47). `total`
// rides along everywhere but never competes for "slowest" — it only sums
// the others.
const STAGES = ["capture", "ocr", "translate", "fused"];

// A support tool left open for hours must not grind the DOM down: rows
// beyond this drop oldest-first. The summary keeps every pass since open
// (or the last Clear).
const MAX_ROWS = 500;

// The exportable history (ticket 49): a structured mirror of everything
// the window showed, kept beside the DOM so Export… can rebuild the
// session as text without reading it back out of the DOM. Bounded like
// the rows but much larger — entries are cheap; when the cap trips, the
// oldest entries drop and the export header's covered range says what
// actually remains.
const MAX_HISTORY = 5000;
const history = [];
// The export header's "session started at" — the window's opening (or,
// after a Clear — ticket 52 — the clear): a post-clear export's covered
// range begins there.
let openedAtMs = Date.now();

// Rolling per-stage stats since the window opened — or the last Clear.
// Capture counts every pass (it runs even on unchanged ticks — it can be
// the slow stage on a large zone); ocr/translate/fused count only the
// passes where they ran — the backend reports null on passes where a
// stage never ran (ticket 50), and a translate min of 0ms is honest: a
// Glossary-pinned fire genuinely costs ~nothing.
const stats = {
  capture: freshStats(),
  ocr: freshStats(),
  translate: freshStats(),
  fused: freshStats(),
  total: freshStats(),
};
let passCount = 0;

// The row currently accumulating a run of consecutive unchanged ticks for
// its origin (a fast-path pass costs capture only, so an idle screen
// collapses into "unchanged ×N · capture ~10ms" instead of flooding the
// window). Any other row breaks the run.
let unchangedRun = null;

// The history entry accumulating the same run (ticket 49) — bumped in
// lockstep with `unchangedRun`, so the export shows one `unchanged ×N`
// block per run, like the window. Any other entry breaks it.
let openHistoryRun = null;

function freshStats() {
  return { n: 0, sum: 0, min: Infinity, max: 0 };
}

// Records one stage measurement into the rolling summary. A 0ms reading
// is not a datapoint: it is the Glossary short-circuit translating in no
// measurable time (ticket 50 kept those honest on the per-pass rows —
// they still show `translate 0ms`). Charting them would pin min to 0 and
// drag the average down — the same misleading summary removing the
// no-fire passes fixed. Reported at the user's ask: only readings above
// 0ms are charted.
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

// The run key both mirrors of the unchanged collapse key on (kind + id —
// zone names are unique per target, this is belt-and-braces).
function originRunKey(event) {
  return `${event.origin.kind}:${event.origin.id ?? ""}`;
}

function clockTime(atMs) {
  return new Date(atMs).toLocaleTimeString();
}

// Simple pin-to-bottom auto-scroll: follow the stream unless the user
// scrolled up to inspect (then don't yank them back down).
function isPinned() {
  return rowsEl.scrollTop + rowsEl.clientHeight >= rowsEl.scrollHeight - 32;
}

function trimRows() {
  while (rowsEl.children.length > MAX_ROWS) {
    const dropped = rowsEl.firstElementChild;
    if (dropped === unchangedRun) unchangedRun = null;
    dropped.remove();
  }
}

function appendPinned(row) {
  const pinned = isPinned();
  rowsEl.append(row);
  trimRows();
  if (pinned) rowsEl.scrollTop = rowsEl.scrollHeight;
}

function appendRow(row) {
  unchangedRun = null;
  appendPinned(row);
}

function chip(name, ms) {
  const span = document.createElement("span");
  span.className = "ms" + (name === "total" ? " total" : "");
  span.dataset.stage = name;
  span.dataset.ms = String(ms);
  // The stage names are display text (ticket 62) — routed like every
  // other label; the keys stay stable ("debug.stage.capture", …).
  const label = document.createElement("span");
  label.className = "name";
  label.textContent = `${t(`debug.stage.${name}`)} `;
  span.append(label, document.createTextNode(`${ms}ms`));
  return span;
}

// Marks the largest duration among a row's stage chips (the slow stage of
// the pass, identifiable at a glance). `total` is excluded — it only sums
// the others, so it would win every pass.
function markSlowest(row) {
  let slowest = null;
  let slowestMs = -1;
  for (const el of row.querySelectorAll(".ms")) {
    if (el.dataset.stage === "total") continue;
    const ms = Number(el.dataset.ms);
    if (ms > slowestMs) {
      slowestMs = ms;
      slowest = el;
    }
  }
  slowest?.classList.add("slow");
}

function rowSkeleton(atMs, origin) {
  const li = document.createElement("li");
  const time = document.createElement("span");
  time.className = "time";
  time.textContent = clockTime(atMs);
  const badge = document.createElement("span");
  badge.className = "origin";
  badge.textContent = originLabel(origin);
  li.append(time, badge);
  return li;
}

function timingRow(event) {
  const li = rowSkeleton(event.at_ms, event.origin);
  // A fast-path pass (ticket 48): capture ran, the read never did — its
  // trace lists only the matured fires off the cached reads.
  if (event.read_skipped) {
    const skipped = document.createElement("span");
    skipped.className = "no-read";
    skipped.textContent = t("debug.no_read");
    li.append(skipped);
  }
  // capture always ran; the others only appear when they did.
  for (const name of STAGES) {
    const ms = event[`${name}_ms`];
    if (ms !== null) li.append(chip(name, ms));
  }
  markSlowest(li);
  const blocks = document.createElement("span");
  blocks.className = "blocks";
  blocks.textContent =
    event.blocks_translated === 1
      ? t("debug.blocks_one", { count: event.blocks_translated })
      : t("debug.blocks_many", { count: event.blocks_translated });
  li.append(blocks);
  if (event.reads?.length) li.append(readsDetail(event.reads));
  // Last in the DOM, pushed to the row's right edge by the stylesheet.
  li.append(chip("total", event.total_ms));
  return li;
}

// ---- Read trace (ticket 48): what OCR read, and why each block did or
// ---- didn't translate. Compact counts by default; clicking the pill
// expands the per-block lines.

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

// A read's decision as the loop serializes it (ticket 48): ReadDisposition
// is an internally tagged enum, so a trace's `disposition` is an OBJECT —
// its own `disposition` key carries the tag ("translated_engine",
// "waiting", "gone", …) and a variant's fields sit beside it (waiting:
// elapsed_ms + total_ms; gone: misses). Reading the tag as the field's
// whole value threw on every event that carried reads at all — the row
// never rendered and the export lost the pass (ticket 53).
function dispositionTag(read) {
  return read.disposition.disposition;
}

function countKey(disposition) {
  return disposition.startsWith("translated_") ? "translated" : disposition;
}

function readsDetail(reads) {
  const counts = new Map();
  for (const read of reads) {
    const key = countKey(dispositionTag(read));
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  const parts = COUNT_ORDER.filter((k) => counts.has(k)).map(
    (k) => `${counts.get(k)} ${t(`debug.count.${k}`)}`
  );
  const wrap = document.createElement("span");
  wrap.className = "reads";
  const toggle = document.createElement("button");
  toggle.type = "button";
  toggle.className = "reads-toggle";
  toggle.textContent =
    reads.length === 1
      ? t("debug.reads_toggle_one", { count: reads.length, parts: parts.join(" · ") })
      : t("debug.reads_toggle_many", { count: reads.length, parts: parts.join(" · ") });
  const list = document.createElement("ul");
  list.className = "read-lines";
  list.hidden = true;
  for (const read of reads) list.append(readLine(read));
  toggle.addEventListener("click", () => {
    list.hidden = !list.hidden;
    toggle.classList.toggle("open");
  });
  wrap.append(toggle, list);
  return wrap;
}

function readLine(read) {
  const li = document.createElement("li");
  const words = document.createElement("span");
  words.className = "words";
  words.textContent = read.text === "" ? t("debug.nothing_after_trim") : `“${read.text}”`;
  const reason = document.createElement("span");
  reason.className = `reason ${dispositionTag(read)}`;
  reason.textContent = readReason(read);
  li.append(words, reason);
  return li;
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

// One compact row per run of consecutive unchanged ticks; later ticks in
// the run just bump the count and the capture time shown. Runs key on the
// origin's id (kind + id — zone names are unique per target, this is
// belt-and-braces).
function unchangedRunRow(event) {
  const runKey = originRunKey(event);
  if (unchangedRun !== null && unchangedRun.dataset.origin === runKey) {
    unchangedRun.dataset.count = String(Number(unchangedRun.dataset.count) + 1);
    unchangedRun.querySelector(".count").textContent = unchangedRun.dataset.count;
    unchangedRun.querySelector(".capture-ms").textContent = `${event.capture_ms}ms`;
    unchangedRun.querySelector(".time").textContent = clockTime(event.at_ms);
    return unchangedRun;
  }
  const li = rowSkeleton(event.at_ms, event.origin);
  li.classList.add("unchanged");
  li.dataset.origin = runKey;
  const label = document.createElement("span");
  label.textContent = t("debug.unchanged.label");
  const count = document.createElement("span");
  count.className = "count";
  count.textContent = "1";
  const capture = document.createElement("span");
  capture.textContent = t("debug.unchanged.capture");
  const captureMs = document.createElement("span");
  captureMs.className = "capture-ms";
  captureMs.textContent = `${event.capture_ms}ms`;
  li.append(label, count, capture, captureMs);
  unchangedRun = li;
  return li;
}

function errorRow(event) {
  const li = document.createElement("li");
  li.classList.add("error");
  const time = document.createElement("span");
  time.className = "time";
  time.textContent = clockTime(event.at_ms);
  const stage = document.createElement("span");
  stage.className = "stage";
  stage.textContent = event.stage;
  const message = document.createElement("span");
  message.className = "message";
  message.textContent = event.message;
  li.append(time, stage, message);
  return li;
}

// The slow stage of the session: the largest average among the stages
// that actually ran (total excluded — it only sums the others). Shared by
// the header bar and the export's summary, so both name the same stage.
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

function renderSummary() {
  summaryEl.classList.remove("empty");
  summaryEl.replaceChildren();
  const passes = document.createElement("span");
  passes.className = "passes";
  passes.textContent =
    passCount === 1
      ? t("debug.summary.passes_one", { count: passCount })
      : t("debug.summary.passes_many", { count: passCount });
  summaryEl.append(passes);

  const slowest = slowestStage();
  for (const name of [...STAGES, "total"]) {
    const s = stats[name];
    if (s.n === 0) continue;
    const el = document.createElement("span");
    el.className = "stage-chip" + (name === slowest ? " slow" : "");
    const label = document.createElement("span");
    label.className = "name";
    label.textContent = t(`debug.stage.${name}`);
    const detail = document.createElement("span");
    detail.textContent = t("debug.summary.stage_detail", {
      avg: Math.round(s.sum / s.n),
      min: s.min,
      max: s.max,
      n: s.n,
    });
    el.append(label, detail);
    summaryEl.append(el);
  }
}

// The session controls (Export…, Clear) stay hidden while the window is
// empty — nothing to export, nothing to clear — and appear with the first
// RENDERED event. Keying them to the two handlers (not "any event on the
// stream") keeps the invariant airtight — buttons visible ⟺ a row and a
// history entry exist — no matter what the loop ever forwards here
// (today only timings and errors reach this window; ticket 52; Export's
// rule since ticket 49, now shared).
function showSessionControls() {
  exportBtn.hidden = false;
  clearBtn.hidden = false;
}

function handleTiming(event) {
  showSessionControls();
  passCount += 1;
  record("capture", event.capture_ms);
  for (const name of STAGES.slice(1)) {
    const ms = event[`${name}_ms`];
    if (ms !== null) record(name, ms);
  }
  record("total", event.total_ms);
  renderSummary();

  if (event.read_skipped && event.blocks_translated === 0) {
    // A tick that only captured and checked (the fast path: frame
    // unchanged, no read ran, no translation was produced): collapse
    // consecutive runs so an idle screen costs one compact row, not one
    // row per pass. A pass that DIED before reading (failed capture) is
    // not a skip — it keeps its full row, the Error event beside it
    // naming the cause (ticket 48's explicit read_skipped flag).
    const row = unchangedRunRow(event);
    if (row.parentElement === null) {
      appendPinned(row);
    } else if (isPinned()) {
      rowsEl.scrollTop = rowsEl.scrollHeight;
    }
    recordUnchangedHistory(event);
    return;
  }
  appendRow(timingRow(event));
  recordTimingHistory(event);
}

function handleError(event) {
  showSessionControls();
  appendRow(errorRow(event));
  recordErrorHistory(event);
}

// ---- Ticket 49: the exportable history -------------------------------
// One entry per thing the window showed, pushed in stream order — full
// passes, collapsed unchanged runs (bumped in place), errors. The export
// is a pure function of this array: no DOM reads, so exporting twice
// without new events produces the same text.

function pushHistory(entry) {
  history.push(entry);
  while (history.length > MAX_HISTORY) {
    const dropped = history.shift();
    // Defensive: if the cap drops the open run's entry, the run ends —
    // the next tick must start a fresh entry, not bump a detached one.
    // (The push sites reset openHistoryRun before pushing, so this
    // guards the invariant locally instead of trusting their order.)
    if (dropped === openHistoryRun) openHistoryRun = null;
  }
}

function recordTimingHistory(event) {
  openHistoryRun = null;
  pushHistory({
    kind: "timing",
    atMs: event.at_ms,
    origin: originLabel(event.origin),
    isZone: event.origin.kind === "zone",
    readSkipped: event.read_skipped,
    // `capture` is always set; the others are null when the stage
    // didn't run — the export omits them like the window's chips.
    stages: {
      capture: event.capture_ms,
      ocr: event.ocr_ms,
      translate: event.translate_ms,
      fused: event.fused_ms,
    },
    totalMs: event.total_ms,
    blocks: event.blocks_translated,
    reads: event.reads ?? [],
  });
}

// A tick continuing the open run BUMPS the run's entry (count, latest
// capture ms, latest time) instead of pushing a new one — the export
// shows `unchanged ×N · capture ~Xms` as one block, like the window.
function recordUnchangedHistory(event) {
  const runKey = originRunKey(event);
  if (openHistoryRun !== null && openHistoryRun.runKey === runKey) {
    openHistoryRun.count += 1;
    openHistoryRun.captureMs = event.capture_ms;
    openHistoryRun.atMs = event.at_ms;
    return;
  }
  openHistoryRun = {
    kind: "unchanged",
    runKey,
    origin: originLabel(event.origin),
    isZone: event.origin.kind === "zone",
    count: 1,
    captureMs: event.capture_ms,
    atMs: event.at_ms,
  };
  pushHistory(openHistoryRun);
}

function recordErrorHistory(event) {
  openHistoryRun = null;
  pushHistory({
    kind: "error",
    atMs: event.at_ms,
    stage: event.stage,
    message: event.message,
  });
}

// ---- Ticket 49: the export text ---------------------------------------
// Plain string building (never HTML), reusing the window's own wording —
// readReason verbatim, the summary's same numbers, the same slowest
// marks — so a paste reads like the window does.

function pad2(n) {
  return String(n).padStart(2, "0");
}

// Date-and-time stamps for the header (row times stay clockTime, matching
// the window); fixed-order fields so a paste into a chat parses the same
// everywhere.
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
// the pass's slowest marked (like the row's chips), then total, blocks,
// and the fast-path marker.
function passLine(entry) {
  let slowestName = null;
  let slowestMs = -1;
  for (const name of STAGES) {
    const ms = entry.stages[name];
    if (ms !== null && ms > slowestMs) {
      slowestMs = ms;
      slowestName = name;
    }
  }
  const parts = [exportOrigin(entry)];
  for (const name of STAGES) {
    const ms = entry.stages[name];
    if (ms === null) continue;
    // " ◂" marks the pass's slow stage — punctuation formatting, kept
    // beside the label like the "---"/" · " line joins.
    parts.push(
      t("debug.export.stage_part", { name: t(`debug.stage.${name}`), ms }) +
        (name === slowestName ? " ◂" : ""),
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
// window's expanded lines show, continuation lines aligned under the
// first read.
function readsLines(reads) {
  const prefix = t("debug.export.reads_prefix", { count: reads.length });
  return reads.map((read, i) => {
    const words = read.text === "" ? t("debug.nothing_after_trim") : `“${read.text}”`;
    const indent = i === 0 ? prefix : " ".repeat(prefix.length);
    return `${indent}${words} → ${readReason(read)}`;
  });
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

// ---- Ticket 49: the Export… button ------------------------------------

let exportStatusTimer = null;

// The window's only error surface: success confirms briefly; a failure
// (dialog refused, write failed) stays until the next attempt.
function flashExportStatus(message, isError) {
  exportStatusEl.textContent = message;
  exportStatusEl.classList.toggle("error", isError);
  exportStatusEl.hidden = false;
  if (exportStatusTimer !== null) clearTimeout(exportStatusTimer);
  if (!isError) {
    exportStatusTimer = setTimeout(() => {
      exportStatusEl.hidden = true;
      exportStatusTimer = null;
    }, 4000);
  }
}

function defaultExportName() {
  const d = new Date();
  return `langception-debug-${d.getFullYear()}${pad2(d.getMonth() + 1)}${pad2(d.getDate())}` +
    `-${pad2(d.getHours())}${pad2(d.getMinutes())}${pad2(d.getSeconds())}.txt`;
}

async function exportLog() {
  try {
    const path = await saveFileDialog({
      title: t("debug.export.dialog_title"),
      defaultPath: defaultExportName(),
      filters: [{ name: "Text", extensions: ["txt"] }],
    });
    if (!path) return;
    const version = await getVersion();
    // Snapshot taken here — a pure function of the history array, so two
    // exports without new events in between differ only in `exported at`.
    const text = buildExportText(version);
    await invoke("export_debug_log", { path, contents: text });
    flashExportStatus(t("debug.export.saved"), false);
  } catch (e) {
    flashExportStatus(String(e), true);
  }
}

exportBtn.addEventListener("click", exportLog);

// ---- Ticket 52: the Clear button --------------------------------------
// One click resets every piece of window state the way reopening the
// window would — rows, rolling stats, structured history, both
// unchanged-run accumulators, the pass count, and the opened-at timestamp
// the export header reports — so a new debugging session starts clean
// without the reopen dance. Confirm-free by design: this is a view, not
// config, and Export… exists for keeping a copy first. The loop is not
// touched; events arriving during/after the clear render normally into
// the fresh window.
function clearAll() {
  // Emptying the list also resets the pin-to-bottom state: the next
  // append computes it from an empty scroll box.
  rowsEl.replaceChildren();
  unchangedRun = null; // a run must not continue across a clear
  openHistoryRun = null;
  history.length = 0;
  openedAtMs = Date.now();
  passCount = 0;
  for (const name of [...STAGES, "total"]) stats[name] = freshStats();
  summaryEl.classList.add("empty");
  summaryEl.textContent = t(EMPTY_SUMMARY_KEY);
  // Back to the empty state: nothing to export or clear until the next
  // event renders (showSessionControls unhides them).
  exportBtn.hidden = true;
  clearBtn.hidden = true;
  // The transient status doubles as the click's feedback — the minimal
  // nicety the brief floated, reusing the export status's flash.
  flashExportStatus(t("debug.clear.cleared"), false);
}
clearBtn.addEventListener("click", clearAll);

window.addEventListener("DOMContentLoaded", async () => {
  // Ticket 62: the window has no status payload to ride — the one-invoke
  // bootstrap seam resolves the language before the first row renders.
  try {
    await window.I18n.init(await invoke("get_ui_language"));
  } catch {
    await window.I18n.init("en"); // offline of the backend: English
  }

  await listen("loop-event", (event) => {
    const payload = event.payload;
    if (payload.type === "timing") {
      handleTiming(payload);
    } else if (payload.type === "error") {
      handleError(payload);
    }
    // "translated" events stay with the Results Window; the debug view is
    // timings + errors only. The session controls appear with the first
    // rendered event (showSessionControls, called by the two handlers).
  });

  // Ticket 61: a UI-Language saved in Settings reaches this window on the
  // same engine-state broadcast — re-resolve the dictionaries so the
  // static labels follow immediately (rows already rendered keep their
  // original language, frozen artifacts).
  await listen("engine-state", (event) => {
    const next = event.payload && event.payload.uiLanguage;
    if (next && next !== window.I18n.language()) window.I18n.init(next);
  });
});
