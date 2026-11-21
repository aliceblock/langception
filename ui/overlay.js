// Langception In-place Overlay — vanilla JS against the Tauri global API.
// The shell routes one "overlay-update" per translated Text Block with the
// block rect in PHYSICAL px relative to this monitor; blocks are keyed by
// (zone id, orchestrator block key) and their divs are reused in place, so
// updates never flicker. "overlay-remove"/"overlay-clear" drop stale blocks,
// "overlay-style" restyles everything live. No build step.

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

// zone id → (block key → { el, textEl, boxW, boxH })
const zones = new Map();

function createBlock() {
  const el = document.createElement("div");
  el.className = "overlay-block";
  const textEl = document.createElement("span");
  textEl.className = "overlay-text";
  el.append(textEl);
  document.body.append(el);
  const entry = { el, textEl, boxW: 0, boxH: 0 };
  applyStyleTo(entry);
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

window.addEventListener("DOMContentLoaded", () => {
  listen("overlay-update", (event) => updateBlock(event.payload));
  listen("overlay-remove", (event) => removeBlocks(event.payload));
  listen("overlay-clear", (event) => clearBlocks(event.payload));
  listen("overlay-style", (event) => {
    stylePushed = true;
    style = { ...DEFAULT_STYLE, ...event.payload };
    applyStyleToAll();
  });

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
