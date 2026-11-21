// Langception In-place Overlay — vanilla JS against the Tauri global API.
// The shell routes one "overlay-update" per translated Text Block with the
// block rect in PHYSICAL px relative to this monitor; blocks are keyed by
// (zone id, orchestrator block key) and their divs are reused in place, so
// updates never flicker. "overlay-remove"/"overlay-clear" drop stale blocks,
// "overlay-style" restyles everything live. "overlay-adjust" toggles Adjust
// Mode: click-through off, Capture Zone rectangles draggable/resizable, and
// (full-screen mode) pills clickable to mark Excluded Blocks. No build step.

const { invoke } = window.__TAURI__.core;
// Targeted listeners, NOT window.__TAURI__.event.listen: the plain event API
// registers EventTarget::Any, which Tauri delivers to on every emit
// regardless of emit_to targeting — with two overlays both pages rendered
// every block (pills on all monitors). WebviewWindow.listen matches this
// page's label only, so the shell's per-monitor routing holds.
const overlayWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
const listen = overlayWindow.listen.bind(overlayWindow);

// Below this font size, shrink-to-fit stops — a slightly overflowing pill
// beats unreadably small text.
const MIN_FONT_PX = 8;
// A minimum pill width so tiny OCR boxes (single short words) don't force
// the text into a sliver.
const MIN_BOX_PX = 24;
// Wrapped fallback may reach twice the OCR box height before shrinking.
const HEIGHT_BUDGET_FACTOR = 2;
// Dragged/resized zones stay at least this big (CSS px), matching the
// picker's minimum.
const MIN_ZONE_PX = 8;

const DEFAULT_STYLE = {
  fontFamily: "Noto Sans Thai",
  fontSize: 16,
  color: "#ffffff",
  outlineWidth: 2,
  outlineColor: "#000000",
  background: true,
  backgroundOpacity: 0.6,
};

let style = { ...DEFAULT_STYLE };
let stylePushed = false; // an overlay-style event beats the initial pull

// zone id → (block key → { el, textEl, boxW, boxH, rect, text })
// `rect` is the block's raw OCR box in physical px relative to this
// monitor; `text` the source text. Both feed Excluded Block clicks.
const zones = new Map();

// Adjust Mode state (this page). `zonesRects` maps zone id → CSS-px
// geometry; the DOM nodes live in #adjust-layer.
const adjust = {
  active: false,
  fullscreen: false,
  originX: 0, // this monitor's global physical origin
  originY: 0,
  rects: new Map(), // zone id → { x, y, w, h, name }
};

// ---- Translation pills ----

function createBlock() {
  const el = document.createElement("div");
  el.className = "overlay-block";
  const textEl = document.createElement("span");
  textEl.className = "overlay-text";
  el.append(textEl);
  document.body.append(el);
  const entry = { el, textEl, boxW: 0, boxH: 0, rect: null, text: "" };
  applyStyleTo(entry);
  // In Adjust Mode over full-screen mode, clicking a pill marks the block
  // excluded. The listener exists from creation; the check inside keeps it
  // inert outside that combination.
  el.addEventListener("click", () => excludeClickedBlock(entry));
  return entry;
}

function applyStyleTo(entry) {
  const s = style;
  entry.el.style.fontFamily = `"${s.fontFamily}", "Noto Sans Thai", "Segoe UI", sans-serif`;
  entry.el.style.webkitTextStrokeWidth = s.outlineWidth + "px";
  entry.el.style.webkitTextStrokeColor = s.outlineColor;
  entry.el.style.color = s.color;
  if (s.background) {
    entry.el.style.background = `rgba(0, 0, 0, ${s.backgroundOpacity})`;
    entry.el.style.padding = "1px 6px";
    entry.el.style.borderRadius = "4px";
  } else {
    entry.el.style.background = "transparent";
    entry.el.style.padding = "0";
    entry.el.style.borderRadius = "0";
  }
}

function applyStyleToAll() {
  for (const zone of zones.values()) {
    for (const entry of zone.values()) {
      applyStyleTo(entry);
      fitText(entry);
    }
  }
}

// Pill padding must match overlay.css (.overlay-block) — the fit budget
// subtracts it so the text itself fits inside the pill.
const PILL_PAD_X = 12;
const PILL_PAD_Y = 2;

// Auto-shrink-to-fit: the Thai must not sprawl over the screen. Phase 1
// shrinks on a single line until it fits the block's width; if even the
// floor size doesn't fit, phase 2 wraps and keeps the pill within roughly
// twice the original block's height. Measured on the text element's own
// rect so the logic does not depend on container scroll quirks.
function fitText(entry) {
  const { el, textEl, boxW, boxH } = entry;
  if (!boxW) return;
  const padX = style.background ? PILL_PAD_X : 0;
  const padY = style.background ? PILL_PAD_Y : 0;
  const availW = Math.max(boxW - padX, MIN_BOX_PX);
  const budgetH = Math.max(boxH * HEIGHT_BUDGET_FACTOR, MIN_FONT_PX * 1.6) - padY;

  let size = style.fontSize;
  textEl.style.fontSize = size + "px"; // always explicit: the CSS has no default
  textEl.style.whiteSpace = "nowrap";
  while (size > MIN_FONT_PX && textEl.getBoundingClientRect().width > availW) {
    size = Math.max(MIN_FONT_PX, size - 1);
    textEl.style.fontSize = size + "px";
  }
  if (textEl.getBoundingClientRect().width > availW) {
    // Wrapping starts from the configured size again: the largest wrapped
    // size that fits the height budget beats a floor-shrunk single line.
    textEl.style.whiteSpace = "normal";
    size = style.fontSize;
    textEl.style.fontSize = size + "px";
    while (size > MIN_FONT_PX && textEl.getBoundingClientRect().height > budgetH) {
      size = Math.max(MIN_FONT_PX, size - 1);
      textEl.style.fontSize = size + "px";
    }
  }
}

function updateBlock(payload) {
  if (!payload.translation || !payload.translation.trim()) return;

  const dpr = window.devicePixelRatio || 1;
  let zone = zones.get(payload.originId);
  if (!zone) {
    zone = new Map();
    zones.set(payload.originId, zone);
  }
  let entry = zone.get(payload.key);
  if (!entry) {
    entry = createBlock();
    zone.set(payload.key, entry);
  }

  entry.rect = { x: payload.x, y: payload.y, width: payload.width, height: payload.height };
  entry.text = payload.text || payload.translation;
  entry.boxW = Math.max(payload.width / dpr, MIN_BOX_PX);
  entry.boxH = Math.max(payload.height / dpr, MIN_FONT_PX);
  entry.el.style.left = payload.x / dpr + "px";
  entry.el.style.top = payload.y / dpr + "px";
  entry.el.style.maxWidth = entry.boxW + "px";
  entry.textEl.textContent = payload.translation;
  fitText(entry);
}

function removeBlocks(payload) {
  const zone = zones.get(payload.originId);
  if (!zone) return;
  for (const key of payload.keys) {
    const entry = zone.get(key);
    if (entry) {
      entry.el.remove();
      zone.delete(key);
    }
  }
  if (zone.size === 0) zones.delete(payload.originId);
}

function clearBlocks(payload) {
  const drop = (zone) => {
    for (const entry of zone.values()) entry.el.remove();
  };
  if (payload.originId == null) {
    for (const zone of zones.values()) drop(zone);
    zones.clear();
  } else {
    const zone = zones.get(payload.originId);
    if (zone) {
      drop(zone);
      zones.delete(payload.originId);
    }
  }
}

// ---- Excluded Blocks (Adjust Mode over full-screen mode) ----

async function excludeClickedBlock(entry) {
  if (!adjust.active || !adjust.fullscreen || !entry.rect) return;
  try {
    await invoke("exclude_block", {
      x: adjust.originX + entry.rect.x,
      y: adjust.originY + entry.rect.y,
      width: entry.rect.width,
      height: entry.rect.height,
      label: entry.text,
    });
    // The pill goes away at once; the loop stops re-translating the
    // position on its next pass.
    entry.el.remove();
    lookup: for (const [zoneId, zone] of zones) {
      for (const [key, candidate] of zone) {
        if (candidate === entry) {
          zone.delete(key);
          if (zone.size === 0) zones.delete(zoneId);
          break lookup;
        }
      }
    }
  } catch (err) {
    showAdjustNote(String(err), true);
  }
}

// ---- Adjust Mode: zone rectangles ----

// The handle directions, in CSS cursor terms.
const HANDLE_DIRS = [
  ["n", "ns-resize"],
  ["s", "ns-resize"],
  ["e", "ew-resize"],
  ["w", "ew-resize"],
  ["ne", "nesw-resize"],
  ["nw", "nwse-resize"],
  ["se", "nwse-resize"],
  ["sw", "nesw-resize"],
];

let adjustLayer = null;
let adjustNote = null;
let dragging = null; // { kind: "move"|"resize", dir, id, startMouse, startRect }

function ensureAdjustLayer() {
  if (!adjustLayer) {
    adjustLayer = document.createElement("div");
    adjustLayer.id = "adjust-layer";
    adjustNote = document.createElement("div");
    adjustNote.id = "adjust-note";
    adjustNote.classList.add("hidden");
    adjustLayer.append(adjustNote);
    document.body.append(adjustLayer);
  }
  return adjustLayer;
}

function showAdjustNote(text, isError) {
  if (!adjustNote) return;
  adjustNote.textContent = text;
  adjustNote.classList.toggle("error", Boolean(isError));
  adjustNote.classList.remove("hidden");
}

// Rebuilds the zone rectangles from `adjust.rects` (CSS px). Called on
// enter, on payload refresh (zone moved by the loop config), and after a
// drag commits.
function renderZoneRects() {
  const layer = ensureAdjustLayer();
  for (const child of [...layer.querySelectorAll(".zone-rect")]) child.remove();
  for (const [id, rect] of adjust.rects) {
    const el = document.createElement("div");
    el.className = "zone-rect";
    el.dataset.id = id;

    const label = document.createElement("span");
    label.className = "zone-label";
    label.textContent = rect.name;
    el.append(label);

    for (const [dir, cursor] of HANDLE_DIRS) {
      const handle = document.createElement("div");
      handle.className = `zone-handle handle-${dir}`;
      handle.style.cursor = cursor;
      handle.addEventListener("mousedown", (e) => beginZoneDrag(e, id, "resize", dir));
      el.append(handle);
    }

    el.addEventListener("mousedown", (e) => {
      if (e.target !== el) return; // a handle grabbed it
      beginZoneDrag(e, id, "move", null);
    });

    layer.append(el);
    placeZoneRect(el, rect);
  }
}

function placeZoneRect(el, rect) {
  el.style.left = rect.x + "px";
  el.style.top = rect.y + "px";
  el.style.width = rect.w + "px";
  el.style.height = rect.h + "px";
}

function beginZoneDrag(e, id, kind, dir) {
  if (e.button !== 0 || dragging) return;
  e.preventDefault();
  e.stopPropagation();
  const rect = adjust.rects.get(id);
  if (!rect) return;
  dragging = {
    kind,
    dir,
    id,
    startMouse: { x: e.clientX, y: e.clientY },
    startRect: { ...rect },
  };
}

document.addEventListener("mousemove", (e) => {
  if (!dragging) return;
  const rect = adjust.rects.get(dragging.id);
  if (!rect) return;
  const dx = e.clientX - dragging.startMouse.x;
  const dy = e.clientY - dragging.startMouse.y;
  const s = dragging.startRect;

  if (dragging.kind === "move") {
    rect.x = s.x + dx;
    rect.y = s.y + dy;
  } else {
    // Resize: the grabbed edges follow the mouse, clamped to the minimum.
    let { x, y, w, h } = s;
    const dir = dragging.dir;
    if (dir.includes("e")) w = Math.max(MIN_ZONE_PX, s.w + dx);
    if (dir.includes("s")) h = Math.max(MIN_ZONE_PX, s.h + dy);
    if (dir.includes("w")) {
      w = Math.max(MIN_ZONE_PX, s.w - dx);
      x = s.x + (s.w - w);
    }
    if (dir.includes("n")) {
      h = Math.max(MIN_ZONE_PX, s.h - dy);
      y = s.y + (s.h - h);
    }
    rect.x = x;
    rect.y = y;
    rect.w = w;
    rect.h = h;
  }
  const el = adjustLayer.querySelector(`.zone-rect[data-id="${CSS.escape(dragging.id)}"]`);
  if (el) placeZoneRect(el, rect);
});

document.addEventListener("mouseup", async (e) => {
  if (!dragging || e.button !== 0) return;
  const { kind, id, startRect } = dragging;
  const rect = adjust.rects.get(id);
  dragging = null;
  if (!rect) return;

  const moved =
    rect.x !== startRect.x ||
    rect.y !== startRect.y ||
    rect.w !== startRect.w ||
    rect.h !== startRect.h;
  if (!moved) return;

  const dpr = window.devicePixelRatio || 1;
  const global = {
    x: adjust.originX + Math.round(rect.x * dpr),
    y: adjust.originY + Math.round(rect.y * dpr),
    width: Math.max(1, Math.round(rect.w * dpr)),
    height: Math.max(1, Math.round(rect.h * dpr)),
  };
  showAdjustNote("Saving…", false);
  try {
    await invoke("update_zone_region", { id, ...global });
    // The refreshed payload re-syncs the rect; nothing else to do here.
  } catch (err) {
    // Revert to the shell's view of the zone on failure.
    adjust.rects.set(id, { ...startRect });
    const el = adjustLayer.querySelector(`.zone-rect[data-id="${CSS.escape(id)}"]`);
    if (el) placeZoneRect(el, startRect);
    showAdjustNote(String(err), true);
  }
});

function applyAdjustPayload(payload) {
  const dpr = window.devicePixelRatio || 1;
  adjust.active = payload.active;
  adjust.fullscreen = payload.fullscreen;
  adjust.originX = payload.originX;
  adjust.originY = payload.originY;
  document.body.classList.toggle("adjust", payload.active);

  if (payload.active) {
    adjust.rects = new Map(
      payload.zones.map((z) => [
        z.id,
        {
          x: z.x / dpr,
          y: z.y / dpr,
          w: z.width / dpr,
          h: z.height / dpr,
          name: z.name,
        },
      ]),
    );
    ensureAdjustLayer();
    renderZoneRects();
    showAdjustNote(
      payload.fullscreen
        ? "Adjust Mode — click a block to exclude it (it will not be translated again) · Alt+A to exit"
        : "Adjust Mode — drag zones to move, handles to resize · Alt+A to exit",
      false,
    );
  } else {
    adjust.rects = new Map();
    if (adjustLayer) {
      for (const child of [...adjustLayer.querySelectorAll(".zone-rect")]) child.remove();
      adjustNote.classList.add("hidden");
    }
  }
}

document.addEventListener("keydown", (e) => {
  // Esc is the visible affordance; Alt+A works too (global hotkey).
  if (e.key === "Escape" && adjust.active) {
    invoke("toggle_adjust_mode").catch(() => {});
  }
});

window.addEventListener("DOMContentLoaded", () => {
  listen("overlay-update", (event) => updateBlock(event.payload));
  listen("overlay-remove", (event) => removeBlocks(event.payload));
  listen("overlay-clear", (event) => clearBlocks(event.payload));
  listen("overlay-style", (event) => {
    stylePushed = true;
    style = { ...DEFAULT_STYLE, ...event.payload };
    applyStyleToAll();
  });
  listen("overlay-adjust", (event) => applyAdjustPayload(event.payload));

  // Initial style: pulled once so a freshly created overlay is styled
  // before its first block arrives. A live push (settings panel) wins.
  invoke("get_text_style")
    .then((pulled) => {
      if (!stylePushed) {
        style = { ...DEFAULT_STYLE, ...pulled };
        applyStyleToAll();
      }
    })
    .catch(() => {}); // defaults already in place

  // Blocks laid out before the bundled font finished loading were measured
  // with fallback metrics — re-fit everything once it is ready.
  document.fonts.ready.then(() => applyStyleToAll());
});
