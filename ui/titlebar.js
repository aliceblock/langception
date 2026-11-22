// Frameless window titlebar (ticket 16): fills the .titlebar-buttons slot
// with the min/close controls and wires them to the Tauri window API.
// Loaded by the two frameless windows, Results and Settings. close() routes
// through the Rust CloseRequested handler, which hides the window instead
// of destroying it (tray-first).
(function () {
  const slot = document.querySelector(".titlebar-buttons");
  if (!slot) return;
  slot.innerHTML =
    '<button class="tb-btn" title="Minimize" aria-label="Minimize">' +
    '<svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 5h10" stroke="currentColor" stroke-width="1"/></svg>' +
    "</button>" +
    '<button class="tb-btn close" title="Hide to tray" aria-label="Close window">' +
    '<svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 0l10 10M10 0L0 10" stroke="currentColor" stroke-width="1"/></svg>' +
    "</button>";

  // The v2 global bundle exposes getCurrentWebviewWindow on
  // __TAURI__.webviewWindow (not __TAURI__.window).
  const thisWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
  const [minBtn, closeBtn] = slot.querySelectorAll(".tb-btn");
  minBtn.addEventListener("click", () => thisWindow.minimize());
  closeBtn.addEventListener("click", () => thisWindow.close());
})();
