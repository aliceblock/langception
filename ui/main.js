// Langception Results Window — vanilla JS against the Tauri global API.
// Events from the translation loop: "loop-event" (translated | error) and
// "engine-state" (status snapshot with the capture target, zone list,
// Excluded Blocks + hotkeys). Zone/exclusion management (rename/delete/
// clear) and mode switching invoke the shell directly; config editing lives
// in the Settings window (ticket 06). No build step.
// Ticket 62: the window's UI language rides the get_status payload (the
// bootstrap seam) into I18n.init; every user-visible string resolves
// through t().
// Ticket 61 (reopened): the window follows the redesign mock — contextual
// Start/Pause/Stop buttons, an interactive source-language dropdown
// (set_source_language), an always-on-top pin (set_results_on_top), the
// collapsible config section with a per-mode action button, a log toolbar
// with Clear, the Adjust banner, and the footer status bar fed by the new
// ocrEngine/appVersion/overlayVisible status fields.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { writeText } = window.__TAURI__.clipboardManager;
const { t } = window.I18n;

const statusPill = document.getElementById("status-pill");
const privacyChip = document.getElementById("privacy-chip");
const startBtn = document.getElementById("start-btn");
const pauseBtn = document.getElementById("pause-btn");
const pauseLabel = document.getElementById("pause-label");
const pauseIcon = pauseBtn.querySelector(".icon-pause");
const resumeIcon = pauseBtn.querySelector(".icon-resume");
const stopBtn = document.getElementById("stop-btn");
const adjustBtn = document.getElementById("adjust-btn");
const settingsBtn = document.getElementById("settings-btn");
const pinBtn = document.getElementById("pin-btn");
const warningsLine = document.getElementById("warnings-line");
const langBtn = document.getElementById("lang-btn");
const langMenu = document.getElementById("lang-menu");
const langCurrent = document.getElementById("lang-current");
const langOptions = [...document.querySelectorAll(".lang-option")];
const profileChip = document.getElementById("profile-chip");
const profileName = document.getElementById("profile-name");
const errorBanner = document.getElementById("error-banner");
const resultsList = document.getElementById("results");
const emptyState = document.getElementById("empty-state");
const emptyShortcuts = document.getElementById("empty-shortcuts");
const copiedToast = document.getElementById("copied-toast");
const logToolbar = document.getElementById("log-toolbar");
const logCountLabel = document.getElementById("log-count-label");
const clearBtn = document.getElementById("clear-btn");
const configPanelBody = document.getElementById("config-panel-body");
const configToggleBtn = document.getElementById("config-toggle-btn");
const configToggleText = document.getElementById("config-toggle-text");
const configToggleIcon = document.getElementById("config-toggle-icon");
const zoneCountBadge = document.getElementById("zone-count-badge");
const newZoneBtn = document.getElementById("new-zone-btn");
const excludeAreaBtn = document.getElementById("exclude-area-btn");
const modeZones = document.getElementById("mode-zones");
const modeFullscreen = document.getElementById("mode-fullscreen");
const viewZones = document.getElementById("view-zones");
const viewFullscreen = document.getElementById("view-fullscreen");
const monitorSelect = document.getElementById("monitor-select");
const zonesList = document.getElementById("zones-list");
const zonesEmpty = document.getElementById("zones-empty");
const excludedTitle = document.getElementById("excluded-title");
const excludedList = document.getElementById("excluded-list");
const excludedEmpty = document.getElementById("excluded-empty");
const adjustBanner = document.getElementById("adjust-banner");
const adjustDone = document.getElementById("adjust-done");
const footerEngineName = document.getElementById("footer-engine-name");
const footerOverlayValue = document.getElementById("footer-overlay-value");
const footerVersion = document.getElementById("footer-version");

// Frameless window controls (min/close) are wired by titlebar.js, loaded
// before this script.

let running = false;
let paused = false;
let onTop = false;
let language = "en";
let configCollapsed = false;
let translationCount = 0;
let zones = [];
let excludedBlocks = [];
let monitors = [];
let mode = "zones";
let fullscreenMonitor = "";
let hotkeys = {
  toggle: "Alt+Shift+T",
  newZone: "Alt+Shift+Z",
  pause: "Alt+Shift+P",
  adjust: "Alt+Shift+A",
  oneShot: "Alt+Shift+Q",
};
let lastError = null; // { stage, message } — stays visible until the next success

function renderStatus(status) {
  running = status.running;
  paused = status.paused;
  language = status.language || "en";
  zones = status.zones || [];
  excludedBlocks = status.excludedBlocks || [];
  monitors = status.monitors || [];
  mode = status.mode || "zones";
  fullscreenMonitor = status.fullscreenMonitor || "";
  onTop = !!status.resultsOnTop;
  if (status.hotkeys) hotkeys = status.hotkeys;

  if (status.configError) {
    statusPill.textContent = t("results.status.config_error");
    statusPill.className = "pill error";
  } else if (running && paused) {
    statusPill.textContent = t("results.status.paused");
    statusPill.className = "pill paused";
  } else if (running) {
    statusPill.textContent = t("results.status.running");
    statusPill.className = "pill running";
  } else {
    statusPill.textContent = t("results.status.stopped");
    statusPill.className = "pill stopped";
  }

  // The mock's contextual playback trio: Start owns the stopped state,
  // Pause/Resume + Stop own the running one. The paused sub-state swaps
  // the Pause button's icon and label for the resume side.
  startBtn.classList.toggle("hidden", running);
  pauseBtn.classList.toggle("hidden", !running);
  stopBtn.classList.toggle("hidden", !running);
  pauseBtn.disabled = !running;
  pauseLabel.textContent = paused ? t("results.pause.resume") : t("results.pause.pause");
  pauseIcon.classList.toggle("hidden", paused);
  resumeIcon.classList.toggle("hidden", !paused);

  // The mock's Adjust banner replaces the old titlebar chip: the amber
  // pill floats over the workspace while Adjust Mode is on.
  adjustBanner.classList.toggle("hidden", !status.adjustMode);

  // Privacy badge (ticket 06): shown whenever the active Translate Engine
  // sends screen text off-machine (same data as the tray tooltip).
  const privacyEngine = status.privacyEngine;
  privacyChip.classList.toggle("hidden", !privacyEngine);
  if (privacyEngine) {
    privacyChip.title = t("results.privacy_chip.cloud_title", { engine: privacyEngine });
  }

  // Ticket 19: the title bar tells you at a glance what the loop is
  // running with — the language pair, and the Profile while one is matched
  // to the foreground app (ticket 07: its settings are what the loop uses).
  langCurrent.textContent = `${(status.language || "en").toUpperCase()} → ${(
    status.targetLanguage || "th"
  ).toUpperCase()}`;
  for (const option of langOptions) {
    option.classList.toggle("current", option.dataset.lang === status.language);
  }
  const activeProfile = (status.profiles || []).find(
    (p) => p.id && p.id === status.activeProfileId,
  );
  if (activeProfile) {
    profileName.textContent = activeProfile.name;
    profileChip.classList.remove("hidden");
  } else {
    profileChip.classList.add("hidden");
  }

  // The pin mirrors the same results_on_top flag the General tab saves.
  pinBtn.classList.toggle("pinned", onTop);
  pinBtn.title = t("results.pin.title");

  // The empty-state Start CTA repeats the header's start side; the card is
  // gone once translations exist, but a slow first pass leaves it visible
  // while the loop already runs — disabled, it can't double-start.
  const emptyStartBtn = document.getElementById("empty-start");
  if (emptyStartBtn) emptyStartBtn.disabled = running;

  // Footer status bar (the mock's): the configured OCR reader (the vision
  // reading mode reads with the engine instead), whether the overlays are
  // up, and the app version.
  footerEngineName.textContent = t(
    status.reading === "fused" ? "results.footer.engine.fused" : engineKey(status.ocrEngine),
  );
  footerOverlayValue.textContent = t(
    status.overlayVisible ? "results.footer.overlay_on" : "results.footer.overlay_off",
  );
  footerVersion.textContent = t("results.footer.version", { version: status.appVersion });

  // Ticket 18: the dense config line is gone — engine, cadence, hold,
  // glossary count and the config path live in the Settings tabs and the
  // tray tooltip. A config error surfaces in the warnings strip instead
  // (with the path — that's the file to fix); warnings stay
  // only-when-present.
  const warnings = [...(status.warnings || [])];
  if (status.configError) {
    warnings.unshift(t("results.config_broken", { path: status.configPath }));
  }
  if (warnings.length > 0) {
    warningsLine.textContent = warnings.join("\n");
    warningsLine.classList.remove("hidden");
  } else {
    warningsLine.classList.add("hidden");
  }

  // Ticket 61 (redesign empty-state card): one shortcut row per action —
  // the action label on the left, the hotkey rendered as pressed keys
  // (<kbd>) on the right. The hotkeys stay dynamic (live-rebindable from
  // Settings), same as everywhere else in this file. Label-only strings
  // (results.empty_shortcut.*) keep the {hotkey} interpolation out of the
  // sentence so the keycaps can carry it instead.
  emptyShortcuts.replaceChildren(
    shortcutRow(t("results.empty_shortcut.zone"), hotkeys.newZone),
    shortcutRow(t("results.empty_shortcut.oneshot"), hotkeys.oneShot),
    shortcutRow(t("results.empty_shortcut.toggle"), hotkeys.toggle),
    shortcutRow(t("results.empty_shortcut.pause"), hotkeys.pause),
    shortcutRow(t("results.empty_shortcut.adjust"), hotkeys.adjust),
  );

  renderModeControls();
  renderZones();
  renderExcluded();
  renderErrorBanner();
}

// The footer's OCR-engine label: the config ids map to their own i18n
// keys (engine names stay Latin in every language, like the Settings
// options' brand names).
function engineKey(id) {
  switch (id) {
    case "pp-ocrv6-tiny": return "results.footer.engine.tiny";
    case "pp-ocrv6-small": return "results.footer.engine.small";
    case "pp-ocrv6-medium": return "results.footer.engine.medium";
    default: return "results.footer.engine.windows";
  }
}

// One row of the empty-state shortcut card: the label, then the hotkey's
// modifier/key parts as individual keycaps joined by "+" glyphs.
function shortcutRow(label, hotkey) {
  const row = document.createElement("div");
  row.className = "shortcut-row";

  const name = document.createElement("span");
  name.textContent = label;

  row.append(name, hotkeyKeys(hotkey));
  return row;
}

// A hotkey as pressed keys: <kbd> parts joined by "+" glyphs.
function hotkeyKeys(hotkey) {
  const keys = document.createElement("span");
  keys.className = "shortcut-keys";
  hotkey.split("+").forEach((part, index) => {
    if (index > 0) {
      const plus = document.createElement("span");
      plus.className = "shortcut-plus";
      plus.textContent = "+";
      keys.append(plus);
    }
    const key = document.createElement("kbd");
    key.textContent = part;
    keys.append(key);
  });
  return keys;
}

// Inline Lucide SVGs for JS-built rows (zone trash, exclusion ✕): the
// exact path data from lucide-static, same as the markup icons.
const ICONS = {
  trash:
    '<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M10 11v6"/><path d="M14 11v6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6"/><path d="M3 6h18"/><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>',
  x: '<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M18 6 6 18"/><path d="m6 6 12 12"/></svg>',
  crosshair:
    '<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="10"/><line x1="22" x2="18" y1="12" y2="12"/><line x1="6" x2="2" y1="12" y2="12"/><line x1="12" x2="12" y1="6" y2="2"/><line x1="12" x2="12" y1="22" y2="18"/></svg>',
  copy: '<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>',
};

function iconEl(name) {
  const span = document.createElement("span");
  span.innerHTML = ICONS[name];
  return span.firstElementChild;
}

// ---- Capture Target (zones vs full-screen + monitor) ----

function renderModeControls() {
  modeZones.checked = mode === "zones";
  modeFullscreen.checked = mode === "fullscreen";
  // The mock's single config section: the view follows the mode, and so
  // does the action button (New Zone in zones mode, Exclude Area in
  // full-screen — which enters Adjust Mode, the real exclusion flow).
  viewZones.classList.toggle("hidden", mode !== "zones");
  viewFullscreen.classList.toggle("hidden", mode !== "fullscreen");
  excludeAreaBtn.classList.toggle("hidden", mode !== "fullscreen");
  newZoneBtn.classList.toggle("hidden", mode !== "zones");
  fillMonitorSelect();
}

function fillMonitorSelect() {
  // Rebuild only when the option set changed, so the dropdown never resets
  // while the user is picking.
  const names = monitors.map((m) => m.name).join("\u0000");
  if (monitorSelect.dataset.names === names) {
    monitorSelect.value = fullscreenMonitor || primaryName() || "";
    return;
  }
  monitorSelect.dataset.names = names;
  monitorSelect.replaceChildren();
  if (monitors.length === 0) {
    const option = document.createElement("option");
    option.textContent = t("results.monitor.none");
    option.disabled = true;
    monitorSelect.append(option);
    return;
  }
  for (const m of monitors) {
    const option = document.createElement("option");
    option.value = m.name;
    option.textContent = m.primary ? t("results.monitor.primary", { name: m.name }) : m.name;
    monitorSelect.append(option);
  }
  monitorSelect.value = fullscreenMonitor || primaryName() || "";
}

function primaryName() {
  const primary = monitors.find((m) => m.primary);
  return primary ? primary.name : "";
}

async function applyCaptureMode(target, monitor) {
  try {
    await invoke("set_capture_mode", { mode: target, monitor: monitor || "" });
  } catch (e) {
    showError("mode", String(e));
  }
  renderStatus(await invoke("get_status"));
}

// ---- Excluded Blocks ----

function renderExcluded() {
  excludedTitle.textContent = t("results.excluded_title.count", {
    count: excludedBlocks.length,
  });

  excludedList.replaceChildren();
  for (const exclusion of excludedBlocks) {
    excludedList.append(excludedRow(exclusion));
  }

  excludedEmpty.textContent = t("results.excluded.empty_hint");
  excludedEmpty.classList.toggle("hidden", excludedBlocks.length > 0);
}

function excludedRow(exclusion) {
  const li = document.createElement("li");
  li.className = "excluded-row";

  const label = document.createElement("span");
  label.className = "excluded-label";
  label.textContent = exclusion.label || t("results.excluded.unlabeled");
  label.title = exclusion.monitor
    ? t("results.excluded.position_title_monitor", { region: exclusion.region, monitor: exclusion.monitor })
    : t("results.excluded.position_title", { region: exclusion.region });

  const del = document.createElement("button");
  del.className = "zone-delete";
  del.title = t("results.excluded.clear_title");
  del.append(iconEl("x"));
  del.addEventListener("click", async () => {
    del.disabled = true;
    try {
      await invoke("delete_excluded_block", { id: exclusion.id });
      renderStatus(await invoke("get_status"));
    } catch (e) {
      showError("excluded-block", String(e));
    }
  });

  li.append(label, del);
  return li;
}

// Engine errors surface visibly: sticky banner + pill, never silent.
function showError(stage, message) {
  lastError = { stage, message };
  statusPill.textContent = t("results.status.error");
  statusPill.className = "pill error";
  renderErrorBanner();
}

function renderErrorBanner() {
  if (lastError) {
    errorBanner.textContent = t("results.error_banner", {
      stage: lastError.stage,
      message: lastError.message,
    });
    errorBanner.classList.remove("hidden");
  } else {
    errorBanner.classList.add("hidden");
  }
}

// ---- Zone management (the mock's two-line cards) ----

function renderZones() {
  zoneCountBadge.textContent = String(zones.length);

  zonesList.replaceChildren();
  for (const zone of zones) {
    zonesList.append(zoneRow(zone));
  }

  if (zones.length === 0) {
    // The mock's hint, with the live hotkey as keycaps between the two
    // translated halves: "{prefix}<kbd>Alt</kbd>+…{suffix}".
    zonesEmpty.replaceChildren(
      iconEl("crosshair"),
      textSpan(t("results.zones.empty_prefix")),
      hotkeyKeys(hotkeys.newZone),
      textSpan(t("results.zones.empty_suffix")),
    );
    zonesEmpty.classList.remove("hidden");
  } else {
    zonesEmpty.classList.add("hidden");
  }
}

function textSpan(text) {
  const span = document.createElement("span");
  span.textContent = text;
  return span;
}

// Adjust Mode has three entry points (the header button, the banner's
// Done, and the full-screen view's Exclude Area button — entering Adjust
// Mode is how a block gets excluded); they all run the same invoke.
async function toggleAdjust() {
  await invoke("toggle_adjust_mode");
  renderStatus(await invoke("get_status"));
}

function zoneRow(zone) {
  const li = document.createElement("li");
  li.className = "zone-row";
  if (!zone.enabled) li.classList.add("zone-disabled");

  const main = document.createElement("span");
  main.className = "zone-main";

  const text = document.createElement("span");
  text.className = "zone-text";

  const name = document.createElement("button");
  name.className = "zone-name";
  name.title = t("results.zone.rename_title");
  name.textContent = zone.enabled ? zone.name : t("results.zone.unplugged", { name: zone.name });
  name.addEventListener("click", () => beginRename(li, zone, name));

  const region = document.createElement("span");
  region.className = "zone-region";
  region.textContent = zone.monitor ? `${zone.region} · ${zone.monitor}` : zone.region;

  text.append(name, region);
  main.append(text);

  const del = document.createElement("button");
  del.className = "zone-delete";
  del.title = t("results.zone.delete_title", { name: zone.name });
  del.append(iconEl("trash"));
  del.addEventListener("click", async () => {
    del.disabled = true;
    try {
      await invoke("delete_zone", { id: zone.id });
      renderStatus(await invoke("get_status"));
    } catch (e) {
      showError("zone", String(e));
    }
  });

  li.append(main, del);
  return li;
}

function beginRename(row, zone, nameBtn) {
  if (row.querySelector("input")) return; // already renaming

  const input = document.createElement("input");
  input.type = "text";
  input.className = "zone-rename";
  input.value = zone.name;
  input.maxLength = 60;

  const finish = async (commit) => {
    const value = input.value.trim();
    input.replaceWith(nameBtn);
    if (!commit || value === zone.name || value.length === 0) return;
    try {
      await invoke("rename_zone", { id: zone.id, name: value });
    } catch (e) {
      showError("zone", String(e));
    }
  };

  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") finish(true);
    if (e.key === "Escape") finish(false);
  });
  input.addEventListener("blur", () => finish(true));

  nameBtn.replaceWith(input);
  input.focus();
  input.select();
}

// ---- Loop events ----

function addEntry(block, translation, origin, atMs) {
  lastError = null;
  renderErrorBanner();
  // No `running` guard: One-shot Capture events legitimately arrive while
  // the loop is stopped — that is the point of One-shot Capture.

  emptyState.classList.add("hidden");
  translationCount += 1;
  logCountLabel.textContent = t("results.history.title", { count: translationCount });
  logToolbar.classList.remove("hidden");

  const li = document.createElement("li");
  li.className = "entry slide-in";
  li.title = t("results.entry.copy_title");

  const meta = document.createElement("div");
  meta.className = "meta";
  const badge = document.createElement("span");
  if (origin && origin.kind === "one_shot") {
    badge.className = "badge oneshot";
    badge.textContent = t("results.entry.oneshot_badge");
  } else {
    badge.className = "badge zone";
    badge.textContent = (origin && origin.name) || t("results.entry.zone_badge");
  }
  const time = document.createElement("span");
  time.className = "entry-time";
  time.textContent = new Date(atMs).toLocaleTimeString();

  // The mock's copy affordance at the card's right edge (the whole card
  // stays clickable-to-copy; this is the visible handle).
  const copy = document.createElement("button");
  copy.className = "copy-btn";
  copy.title = t("results.entry.copy_title");
  copy.append(iconEl("copy"));

  meta.append(badge, time, copy);

  const source = document.createElement("div");
  source.className = "source";
  source.textContent = block.text;

  const thai = document.createElement("div");
  thai.className = "thai";
  thai.textContent = translation;

  li.append(meta, source, thai);
  const doCopy = async () => {
    try {
      await writeText(translation);
      li.classList.add("copied");
      copiedToast.classList.remove("hidden");
      setTimeout(() => {
        li.classList.remove("copied");
        copiedToast.classList.add("hidden");
      }, 900);
    } catch (e) {
      showError("clipboard", String(e));
    }
  };
  li.addEventListener("click", doCopy);
  copy.addEventListener("click", (e) => {
    e.stopPropagation();
    doCopy();
  });

  // Newest first.
  resultsList.prepend(li);
}

// Clear (the mock's log toolbar): wipes the feed client-side — entries
// live only in this window's DOM, the loop keeps running untouched.
function clearEntries() {
  resultsList.replaceChildren();
  translationCount = 0;
  logCountLabel.textContent = t("results.history.title", { count: 0 });
  logToolbar.classList.add("hidden");
  emptyState.classList.remove("hidden");
}

window.addEventListener("DOMContentLoaded", async () => {
  // Ticket 62: the status the window fetches anyway carries the UI
  // Language — the bootstrap seam resolves the dictionaries before the
  // first render writes any t() string.
  const status = await invoke("get_status");
  await window.I18n.init(status.uiLanguage);
  renderStatus(status);

  await listen("loop-event", (event) => {
    const payload = event.payload;
    if (payload.type === "translated") {
      addEntry(payload.block, payload.translation, payload.origin, payload.at_ms);
    } else if (payload.type === "error") {
      showError(payload.stage, payload.message);
    }
  });

  await listen("engine-state", (event) => {
    renderStatus(event.payload);
  });

  startBtn.addEventListener("click", async () => {
    startBtn.disabled = true;
    try {
      try {
        await invoke("start_translation");
      } catch (e) {
        showError("config", String(e));
      }
      renderStatus(await invoke("get_status"));
    } finally {
      startBtn.disabled = false;
    }
  });

  stopBtn.addEventListener("click", async () => {
    stopBtn.disabled = true;
    try {
      await invoke("stop_translation");
      renderStatus(await invoke("get_status"));
    } finally {
      stopBtn.disabled = false;
    }
  });

  pauseBtn.addEventListener("click", async () => {
    pauseBtn.disabled = true;
    try {
      await invoke("toggle_pause_translation");
      renderStatus(await invoke("get_status"));
    } finally {
      pauseBtn.disabled = !running; // renderStatus re-derived the state
    }
  });

  // The language dropdown (the mock's, real): a pair picks the global
  // Source Language; apply_state re-broadcasts engine-state, so the chip
  // and the menu's current mark refresh through the normal path.
  langBtn.addEventListener("click", () => langMenu.classList.toggle("hidden"));
  for (const option of langOptions) {
    option.addEventListener("click", async () => {
      langMenu.classList.add("hidden");
      if (option.dataset.lang === language) return; // already active
      try {
        await invoke("set_source_language", { language: option.dataset.lang });
        renderStatus(await invoke("get_status"));
      } catch (e) {
        showError("language", String(e));
      }
    });
  }
  // Click-away closes the menu (the mock's outside-click handler).
  document.addEventListener("click", (e) => {
    if (!document.getElementById("lang-dropdown").contains(e.target)) {
      langMenu.classList.add("hidden");
    }
  });

  // The pin flips the same results_on_top flag the General tab saves.
  pinBtn.addEventListener("click", async () => {
    pinBtn.disabled = true;
    try {
      await invoke("set_results_on_top", { on: !onTop });
      renderStatus(await invoke("get_status"));
    } catch (e) {
      showError("pin", String(e));
    } finally {
      pinBtn.disabled = false;
    }
  });

  newZoneBtn.addEventListener("click", async () => {
    newZoneBtn.disabled = true;
    try {
      await invoke("open_zone_picker", { mode: "zone" });
    } catch (e) {
      showError("zone", String(e));
    } finally {
      newZoneBtn.disabled = false;
    }
  });

  // Exclude Area (full-screen mode): enters Adjust Mode — in the real app
  // a block is excluded by clicking it on the overlay while Adjust Mode is
  // on, so this button opens that flow.
  excludeAreaBtn.addEventListener("click", async () => {
    excludeAreaBtn.disabled = true;
    try {
      await toggleAdjust();
    } finally {
      excludeAreaBtn.disabled = false;
    }
  });

  // Ticket 61: the empty-state card repeats the two starting actions so a
  // first-run window has them where the eye lands. Same invocations as the
  // header buttons; the Start CTA starts only (the card disappears once
  // translations exist, so its stop side is never reachable).
  document.getElementById("empty-new-zone").addEventListener("click", () => {
    invoke("open_zone_picker", { mode: "zone" }).catch((e) => showError("zone", String(e)));
  });
  document.getElementById("empty-start").addEventListener("click", () => {
    invoke("start_translation").catch((e) => showError("config", String(e)));
  });

  clearBtn.addEventListener("click", clearEntries);

  // The mock's collapsible config body: Hide/Show flips the body and
  // rotates the chevron; the label swaps (both strings i18n'd).
  configToggleBtn.addEventListener("click", () => {
    configCollapsed = !configCollapsed;
    configPanelBody.classList.toggle("hidden", configCollapsed);
    configToggleText.textContent = configCollapsed
      ? t("results.config.show")
      : t("results.config.hide");
    configToggleIcon.style.transform = configCollapsed ? "rotate(180deg)" : "none";
  });

  adjustBtn.addEventListener("click", async () => {
    adjustBtn.disabled = true;
    try {
      await toggleAdjust();
    } finally {
      adjustBtn.disabled = false;
    }
  });

  adjustDone.addEventListener("click", () => {
    toggleAdjust().catch((e) => showError("adjust", String(e)));
  });

  modeZones.addEventListener("change", () => {
    if (modeZones.checked) applyCaptureMode("zones", "");
  });
  modeFullscreen.addEventListener("change", () => {
    if (modeFullscreen.checked) {
      applyCaptureMode("fullscreen", fullscreenMonitor || primaryName() || monitorSelect.value);
    }
  });
  monitorSelect.addEventListener("change", () => {
    if (mode === "fullscreen") applyCaptureMode("fullscreen", monitorSelect.value);
  });

  // The settings window (ticket 06) owns config editing: General, Engines,
  // Glossary, Text Style, Hotkeys, About. It hides on close, so this just
  // surfaces it.
  settingsBtn.addEventListener("click", async () => {
    settingsBtn.disabled = true;
    try {
      await invoke("open_settings_window");
    } catch (e) {
      showError("settings", String(e));
    } finally {
      settingsBtn.disabled = false;
    }
  });
});
