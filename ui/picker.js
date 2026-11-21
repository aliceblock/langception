// Langception Capture Zone picker overlay — vanilla JS against the Tauri
// global API. Mouse-down starts a drag; mouse-up either commits the
// rectangle to the shell (zone or one-shot, whatever the session is) or
// cancels a too-tiny drag. Esc always cancels.

const { invoke } = window.__TAURI__.core;

const selection = document.getElementById("selection");
const bar = document.getElementById("picker-bar");

// Below this (CSS px), a mouse-up counts as a click → cancel.
const MIN_DRAG_PX = 8;

let mode = "zone";
let dragging = false;
let startX = 0;
let startY = 0;

function setBar(text, isError) {
  bar.textContent = text;
  bar.classList.toggle("error", Boolean(isError));
}

function place(x, y, w, h) {
  selection.classList.remove("fullscreen");
  selection.style.left = x + "px";
  selection.style.top = y + "px";
  selection.style.width = w + "px";
  selection.style.height = h + "px";
}

(async () => {
  try {
    mode = await invoke("get_picker_mode");
  } catch {
    mode = "zone";
  }
  setBar(
    mode === "oneshot"
      ? "One-shot Capture — drag a rectangle to translate it once · Esc to cancel"
      : "Drag to draw a Capture Zone · Esc to cancel",
    false,
  );
})();

document.addEventListener("mousedown", (e) => {
  if (e.button !== 0 || dragging) return;
  dragging = true;
  startX = e.clientX;
  startY = e.clientY;
  place(startX, startY, 0, 0);
});

document.addEventListener("mousemove", (e) => {
  if (!dragging) return;
  place(
    Math.min(startX, e.clientX),
    Math.min(startY, e.clientY),
    Math.abs(e.clientX - startX),
    Math.abs(e.clientY - startY),
  );
});

document.addEventListener("mouseup", async (e) => {
  if (!dragging || e.button !== 0) return;
  dragging = false;

  const x = Math.min(startX, e.clientX);
  const y = Math.min(startY, e.clientY);
  const w = Math.abs(e.clientX - startX);
  const h = Math.abs(e.clientY - startY);

  // A click without a drag (or a too-tiny one) cancels the session.
  if (w < MIN_DRAG_PX || h < MIN_DRAG_PX) {
    invoke("cancel_zone_drag").catch(() => {});
    return;
  }

  setBar("Capturing…", false);
  try {
    await invoke("commit_zone_drag", {
      x: Math.round(x),
      y: Math.round(y),
      width: Math.round(w),
      height: Math.round(h),
    });
    // On success the shell closes this window.
  } catch (err) {
    // The session stays open so the error is visible; allow a new drag.
    setBar(String(err), true);
    selection.classList.add("fullscreen");
    selection.style.left = "0px";
    selection.style.top = "0px";
    selection.style.width = "";
    selection.style.height = "";
  }
});

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    invoke("cancel_zone_drag").catch(() => {});
  }
});
