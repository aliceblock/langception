// Langception pill style (tickets 14/24/26) — THE rendering technique for
// translation pills, shared by the In-place Overlay (overlay.js) and the
// Settings Text Style preview (settings.js). Both pages call applyPillStyle
// with a TextStyle object, so what the preview shows is byte-for-byte what
// the overlay draws; neither page keeps its own copy that could drift.
// `sampled` carries the Auto Colors sample (ticket 14); Cover Source
// stretch (ticket 24) stays with the overlay because it needs block
// geometry. Loaded as a plain script before both pages' own JS (no build
// step), so the functions land on `window`.

// "#rrggbb" / "#rgb" → { r, g, b }, or null for anything else.
function hexToRgb(hex) {
  const m = /^#([0-9a-f]{6}|[0-9a-f]{3})$/i.exec(hex || "");
  if (!m) return null;
  const body = m[1];
  const full = body.length === 3 ? [...body].map((c) => c + c).join("") : body;
  return {
    r: parseInt(full.slice(0, 2), 16),
    g: parseInt(full.slice(2, 4), 16),
    b: parseInt(full.slice(4, 6), 16),
  };
}

// Applies font, text outline and the background pill to one element (the
// overlay's block div, or the preview's pill span). `sampled` is the
// block's Auto Colors sample ({ textColor, pillColor } as "#rrggbb") or
// null — null shows the manual colors, which is also what Settings'
// preview always passes: sampling needs capture data it does not have,
// and manual is the documented fallback. Geometry (position, Cover
// Source stretch, shrink-to-fit) stays with the caller: it depends on
// each block's source rect, which a preview does not have.
function applyPillStyle(el, style, sampled) {
  el.style.fontFamily = `"${style.fontFamily}", "Noto Sans Thai", "Segoe UI", sans-serif`;
  el.style.webkitTextStrokeWidth = style.outlineWidth + "px";
  el.style.webkitTextStrokeColor = style.outlineColor;
  // Outline paints before the fill so wide strokes never eat the glyphs
  // (Chromium/WebView2 supports paint-order on HTML text). Set here — not
  // only in the stylesheets — so every consumer gets the technique.
  el.style.paintOrder = "stroke fill";
  el.style.color = sampled ? sampled.textColor : style.color;
  // Reset any Cover Source stretch first, so toggling the option (or the
  // pill) off live leaves no stretched geometry behind.
  el.style.margin = "0";
  el.style.minWidth = "0";
  el.style.minHeight = "0";
  el.style.display = "";
  el.style.alignItems = "";
  el.style.justifyContent = "";
  if (style.background) {
    // The pill color is fixed black unless Auto Colors sampled one; the
    // configured opacity rides on top as the alpha channel.
    const rgb = hexToRgb(sampled ? sampled.pillColor : "#000000") || { r: 0, g: 0, b: 0 };
    el.style.background = `rgba(${rgb.r}, ${rgb.g}, ${rgb.b}, ${style.backgroundOpacity})`;
    el.style.padding = "1px 6px";
    el.style.borderRadius = "4px";
  } else {
    // No pill, no stretch: Cover Source only means anything with a
    // background behind it, so the reset above stands.
    el.style.background = "transparent";
    el.style.padding = "0";
    el.style.borderRadius = "0";
  }
}
