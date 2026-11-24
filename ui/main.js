// Langception Results Window — vanilla JS against the Tauri global API.
// Events from the translation loop: "loop-event" (translated | error) and
// "engine-state" (status snapshot with the capture target, zone list,
// Excluded Blocks + hotkeys). Zone/exclusion management (rename/delete/
// clear) and mode switching invoke the shell directly; config editing lives
// in the Settings window (ticket 06). No build step.
// Ticket 62: the window's UI language rides the get_status payload (the
// bootstrap seam) into I18n.init; every user-visible string resolves
// through t().

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { writeText } = window.__TAURI__.clipboardManager;
const { t } = window.I18n;

const statusPill = document.getElementById("status-pill");
const privacyChip = document.getElementById("privacy-chip");
const adjustChip = document.getElementById("adjust-chip");
const toggleBtn = document.getElementById("toggle-btn");
const pauseBtn = document.getElementById("pause-btn");
const adjustBtn = document.getElementById("adjust-btn");
const settingsBtn = document.getElementById("settings-btn");
const warningsLine = document.getElementById("warnings-line");
const langChip = document.getElementById("lang-chip");
const profileChip = document.getElementById("profile-chip");
const profileName = document.getElementById("profile-name");
const errorBanner = document.getElementById("error-banner");
const resultsList = document.getElementById("results");
const emptyState = document.getElementById("empty-state");
const emptyShortcuts = document.getElementById("empty-shortcuts");
const copiedToast = document.getElementById("copied-toast");
const zonesPanel = document.getElementById("zones-panel");
const zonesTitle = document.getElementById("zones-title");
const zonesList = document.getElementById("zones-list");
const zonesEmpty = document.getElementById("zones-empty");
const newZoneBtn = document.getElementById("new-zone-btn");
const modeZones = document.getElementById("mode-zones");
const modeFullscreen = document.getElementById("mode-fullscreen");
const monitorSelect = document.getElementById("monitor-select");
const monitorRow = document.getElementById("monitor-row");
const modeHint = document.getElementById("mode-hint");
const excludedTitle = document.getElementById("excluded-title");
const excludedList = document.getElementById("excluded-list");
const excludedEmpty = document.getElementById("excluded-empty");

// Frameless window controls (min/close) are wired by titlebar.js, loaded
// before this script.

let running = false;
let paused = false;
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
  zones = status.zones || [];
  excludedBlocks = status.excludedBlocks || [];
  monitors = status.monitors || [];
  mode = status.mode || "zones";
  fullscreenMonitor = status.fullscreenMonitor || "";
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
  adjustChip.classList.toggle("hidden", !status.adjustMode);
  adjustBtn.textContent = status.adjustMode ? t("results.adjust.exit") : t("results.adjust.enter");

  // Privacy badge (ticket 06): shown whenever the active Translate Engine
  // sends screen text off-machine (same data as the tray tooltip).
  const privacyEngine = status.privacyEngine;
  privacyChip.classList.toggle("hidden", !privacyEngine);
  if (privacyEngine) {
    privacyChip.title = t("results.privacy_chip.cloud_title", { engine: privacyEngine });
  }

  toggleBtn.textContent = running ? t("results.toggle.stop") : t("results.toggle.start");
  toggleBtn.classList.toggle("primary", !running);
  // Ticket 61: the redesign's Stop reads as destructive — a rose-tint
  // variant, mirroring the primary amber on the Start side.
  toggleBtn.classList.toggle("danger", running);
  pauseBtn.disabled = !running;
  // The empty-state Start CTA repeats the header's start side; the card is
  // gone once translations exist, but a slow first pass leaves it visible
  // while the loop already runs — disabled, it can't double-start.
  const emptyStartBtn = document.getElementById("empty-start");
  if (emptyStartBtn) emptyStartBtn.disabled = running;
  pauseBtn.textContent = paused ? t("results.pause.resume") : t("results.pause.pause");

  // Ticket 19: the title bar tells you at a glance what the loop is
  // running with — the language pair, and the Profile while one is matched
  // to the foreground app (ticket 07: its settings are what the loop uses).
  langChip.textContent = `${(status.language || "en").toUpperCase()} → ${(
    status.targetLanguage || "th"
  ).toUpperCase()}`;
  const activeProfile = (status.profiles || []).find(
    (p) => p.id && p.id === status.activeProfileId,
  );
  if (activeProfile) {
    profileName.textContent = activeProfile.name;
    profileChip.classList.remove("hidden");
  } else {
    profileChip.classList.add("hidden");
  }

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

// One row of the empty-state shortcut card: the label, then the hotkey's
// modifier/key parts as individual keycaps joined by "+" glyphs.
function shortcutRow(label, hotkey) {
  const row = document.createElement("div");
  row.className = "shortcut-row";

  const name = document.createElement("span");
  name.textContent = label;

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

  row.append(name, keys);
  return row;
}

// ---- Capture Target (zones vs full-screen + monitor) ----

function renderModeControls() {
  modeZones.checked = mode === "zones";
  modeFullscreen.checked = mode === "fullscreen";
  monitorRow.classList.toggle("hidden", mode !== "fullscreen");
  modeHint.textContent =
    mode === "fullscreen"
      ? t("results.mode_hint.fullscreen")
      : t("results.mode_hint.zones");
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

  if (excludedBlocks.length === 0) {
    excludedEmpty.textContent = t("results.excluded.empty_hint", {
      hotkey: hotkeys.adjust,
    });
    excludedEmpty.classList.remove("hidden");
  } else {
    excludedEmpty.classList.add("hidden");
  }
}

function excludedRow(exclusion) {
  const li = document.createElement("li");
  li.className = "zone-row excluded-row";

  const label = document.createElement("span");
  label.className = "excluded-label";
  label.textContent = exclusion.label || t("results.excluded.unlabeled");
  label.title = exclusion.monitor
    ? t("results.excluded.position_title_monitor", { region: exclusion.region, monitor: exclusion.monitor })
    : t("results.excluded.position_title", { region: exclusion.region });

  const meta = document.createElement("span");
  meta.className = "zone-region";
  meta.textContent = exclusion.region;

  const del = document.createElement("button");
  del.className = "zone-delete";
  del.title = t("results.excluded.clear_title");
  del.textContent = "✕";
  del.addEventListener("click", async () => {
    del.disabled = true;
    try {
      await invoke("delete_excluded_block", { id: exclusion.id });
      renderStatus(await invoke("get_status"));
    } catch (e) {
      showError("excluded-block", String(e));
    }
  });

  li.append(label, meta, del);
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

// ---- Zone management (the minimal list) ----

function renderZones() {
  const enabledCount = zones.filter((z) => z.enabled).length;
  zonesTitle.textContent =
    mode === "fullscreen"
      ? t("results.zones_title.fullscreen", { count: zones.length })
      : zones.length !== enabledCount
        ? t("results.zones_title.count_of", { enabled: enabledCount, total: zones.length })
        : t("results.zones_title.count", { count: enabledCount });

  zonesList.replaceChildren();
  for (const zone of zones) {
    zonesList.append(zoneRow(zone));
  }

  if (zones.length === 0) {
    zonesEmpty.textContent = t("results.zones.empty_hint", { hotkey: hotkeys.newZone });
    zonesEmpty.classList.remove("hidden");
  } else {
    zonesEmpty.classList.add("hidden");
  }
}

function zoneRow(zone) {
  const li = document.createElement("li");
  li.className = "zone-row";
  if (!zone.enabled) li.classList.add("zone-disabled");

  const name = document.createElement("button");
  name.className = "zone-name";
  name.title = t("results.zone.rename_title");
  name.textContent = zone.enabled ? zone.name : t("results.zone.unplugged", { name: zone.name });
  name.addEventListener("click", () => beginRename(li, zone, name));

  const region = document.createElement("span");
  region.className = "zone-region";
  region.textContent = zone.monitor ? `${zone.region} · ${zone.monitor}` : zone.region;

  const del = document.createElement("button");
  del.className = "zone-delete";
  del.title = t("results.zone.delete_title", { name: zone.name });
  del.textContent = "✕";
  del.addEventListener("click", async () => {
    del.disabled = true;
    try {
      await invoke("delete_zone", { id: zone.id });
      renderStatus(await invoke("get_status"));
    } catch (e) {
      showError("zone", String(e));
    }
  });

  li.append(name, region, del);
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
  meta.append(badge, time);

  const source = document.createElement("div");
  source.className = "source";
  source.textContent = block.text;

  const thai = document.createElement("div");
  thai.className = "thai";
  thai.textContent = translation;

  li.append(meta, source, thai);
  li.addEventListener("click", async () => {
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
  });

  // Newest first.
  resultsList.prepend(li);
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

  toggleBtn.addEventListener("click", async () => {
    toggleBtn.disabled = true;
    try {
      if (running) {
        await invoke("stop_translation");
      } else {
        try {
          await invoke("start_translation");
        } catch (e) {
          showError("config", String(e));
        }
      }
      renderStatus(await invoke("get_status"));
    } finally {
      toggleBtn.disabled = false;
    }
  });

  pauseBtn.addEventListener("click", async () => {
    pauseBtn.disabled = true;
    try {
      await invoke("toggle_pause_translation");
      renderStatus(await invoke("get_status"));
    } finally {
      pauseBtn.disabled = false;
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

  adjustBtn.addEventListener("click", async () => {
    adjustBtn.disabled = true;
    try {
      await invoke("toggle_adjust_mode");
      renderStatus(await invoke("get_status"));
    } finally {
      adjustBtn.disabled = false;
    }
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
