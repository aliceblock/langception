// Frameless window titlebar (ticket 16): fills the .titlebar-buttons slot
// with the min/close controls and wires them to the Tauri window API.
// Loaded by the two frameless windows, Results and Settings. close() routes
// through the Rust CloseRequested handler, which hides the window instead
// of destroying it (tray-first). Ticket 62: the tooltips/labels come from
// the i18n seam — this script loads before the page's own (which calls
// I18n.init with the window's language), so building waits on I18n.ready.
(async function () {
  await window.I18n.ready;
  const { t } = window.I18n;
  const slot = document.querySelector(".titlebar-buttons");
  if (!slot) return;
  slot.innerHTML =
    `<button class="tb-btn" title="${t("titlebar.minimize")}" aria-label="${t("titlebar.minimize")}">` +
    '<svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 5h10" stroke="currentColor" stroke-width="1"/></svg>' +
    "</button>" +
    `<button class="tb-btn close" title="${t("titlebar.hide_to_tray")}" aria-label="${t("titlebar.close_window")}">` +
    '<svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 0l10 10M10 0L0 10" stroke="currentColor" stroke-width="1"/></svg>' +
    "</button>";

  // The v2 global bundle exposes getCurrentWebviewWindow on
  // __TAURI__.webviewWindow (not __TAURI__.window).
  const thisWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();
  const [minBtn, closeBtn] = slot.querySelectorAll(".tb-btn");
  minBtn.addEventListener("click", () => thisWindow.minimize());
  closeBtn.addEventListener("click", () => thisWindow.close());
})();
