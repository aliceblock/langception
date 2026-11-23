// Langception Debug Log window (tickets 47/48) — vanilla JS against the
// Tauri global API. Purely observational: the page only listens to the
// same "loop-event" stream the Results Window uses, rendering one row per
// pipeline pass with per-stage durations and the per-block read trace
// (what OCR read, and why each block did or didn't translate) plus the
// errors that ride the stream. History starts when the window opens; no
// replay of the past.

const { listen } = window.__TAURI__.event;

const summaryEl = document.getElementById("summary");
const rowsEl = document.getElementById("rows");

// The per-stage durations a timing event can carry (ticket 47). `total`
// rides along everywhere but never competes for "slowest" — it only sums
// the others.
const STAGES = ["capture", "ocr", "translate", "fused"];

// A support tool left open for hours must not grind the DOM down: rows
// beyond this drop oldest-first. The summary keeps every pass since open.
const MAX_ROWS = 500;

// Rolling per-stage stats since the window opened. Capture counts every
// pass (it runs even on unchanged ticks — it can be the slow stage on a
// large zone); ocr/translate/fused count only the passes where they ran.
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

function freshStats() {
  return { n: 0, sum: 0, min: Infinity, max: 0 };
}

function record(stage, ms) {
  const s = stats[stage];
  s.n += 1;
  s.sum += ms;
  s.min = Math.min(s.min, ms);
  s.max = Math.max(s.max, ms);
}

function originLabel(origin) {
  return origin.kind === "zone" ? origin.name : "One-shot";
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
  const label = document.createElement("span");
  label.className = "name";
  label.textContent = `${name} `;
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
    skipped.textContent = "OCR skipped — frame unchanged";
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
  blocks.textContent = `${event.blocks_translated} block${
    event.blocks_translated === 1 ? "" : "s"
  }`;
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

function countKey(disposition) {
  return disposition.startsWith("translated_") ? "translated" : disposition;
}

function readsDetail(reads) {
  const counts = new Map();
  for (const read of reads) {
    const key = countKey(read.disposition);
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  const parts = COUNT_ORDER.filter((k) => counts.has(k)).map(
    (k) => `${counts.get(k)} ${k}`
  );
  const wrap = document.createElement("span");
  wrap.className = "reads";
  const toggle = document.createElement("button");
  toggle.type = "button";
  toggle.className = "reads-toggle";
  toggle.textContent = `${reads.length} read${reads.length === 1 ? "" : "s"}: ${parts.join(" · ")}`;
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
  words.textContent = read.text === "" ? "(nothing after trim)" : `“${read.text}”`;
  const reason = document.createElement("span");
  reason.className = `reason ${read.disposition}`;
  reason.textContent = readReason(read);
  li.append(words, reason);
  return li;
}

// The disposition in plain words — the answer to "why didn't this
// translate?", not the enum's spelling.
function readReason(read) {
  switch (read.disposition) {
    case "translated_engine":
      return "translated · engine";
    case "translated_glossary":
      return "translated · glossary";
    case "translated_fused":
      return "translated · fused read";
    case "translated_cached":
      return "translated · cached read (no OCR this pass)";
    case "suppressed":
      return "suppressed — identical text already shown";
    case "waiting": {
      const left = Math.max(0, read.total_ms - read.elapsed_ms);
      return `waiting ${read.elapsed_ms}/${read.total_ms}ms — ${left}ms to go`;
    }
    case "unchanged":
      return "unchanged — pixels match what is shown";
    case "excluded":
      return "excluded — marked not-to-be-translated";
    case "empty":
      return "empty — no text after trim";
    case "gone":
      return `gone from the read · miss ${read.misses}`;
    default:
      return read.disposition;
  }
}

// One compact row per run of consecutive unchanged ticks; later ticks in
// the run just bump the count and the capture time shown. Runs key on the
// origin's id (kind + id — zone names are unique per target, this is
// belt-and-braces).
function unchangedRunRow(event) {
  const origin = `${event.origin.kind}:${event.origin.id ?? ""}`;
  if (unchangedRun !== null && unchangedRun.dataset.origin === origin) {
    unchangedRun.dataset.count = String(Number(unchangedRun.dataset.count) + 1);
    unchangedRun.querySelector(".count").textContent = unchangedRun.dataset.count;
    unchangedRun.querySelector(".capture-ms").textContent = `${event.capture_ms}ms`;
    unchangedRun.querySelector(".time").textContent = clockTime(event.at_ms);
    return unchangedRun;
  }
  const li = rowSkeleton(event.at_ms, event.origin);
  li.classList.add("unchanged");
  li.dataset.origin = origin;
  const label = document.createElement("span");
  label.textContent = "unchanged ×";
  const count = document.createElement("span");
  count.className = "count";
  count.textContent = "1";
  const capture = document.createElement("span");
  capture.textContent = " · capture ~";
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

function renderSummary() {
  summaryEl.classList.remove("empty");
  summaryEl.replaceChildren();
  const passes = document.createElement("span");
  passes.className = "passes";
  passes.textContent = `${passCount} pass${passCount === 1 ? "" : "es"}`;
  summaryEl.append(passes);

  // The slow stage of the session: the largest average among the stages
  // that actually ran (total excluded — it only sums the others).
  let slowest = null;
  let slowestAvg = -1;
  for (const name of STAGES) {
    const s = stats[name];
    if (s.n > 0 && s.sum / s.n > slowestAvg) {
      slowestAvg = s.sum / s.n;
      slowest = name;
    }
  }
  for (const name of [...STAGES, "total"]) {
    const s = stats[name];
    if (s.n === 0) continue;
    const el = document.createElement("span");
    el.className = "stage-chip" + (name === slowest ? " slow" : "");
    const label = document.createElement("span");
    label.className = "name";
    label.textContent = name;
    const detail = document.createElement("span");
    detail.textContent =
      `${Math.round(s.sum / s.n)} avg · ` +
      `${s.min} min · ${s.max} max ms ×${s.n}`;
    el.append(label, detail);
    summaryEl.append(el);
  }
}

function handleTiming(event) {
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
    return;
  }
  appendRow(timingRow(event));
}

function handleError(event) {
  appendRow(errorRow(event));
}

window.addEventListener("DOMContentLoaded", async () => {
  await listen("loop-event", (event) => {
    const payload = event.payload;
    if (payload.type === "timing") {
      handleTiming(payload);
    } else if (payload.type === "error") {
      handleError(payload);
    }
    // "translated" events stay with the Results Window; the debug view is
    // timings + errors only.
  });
});
