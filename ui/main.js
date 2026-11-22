// Langception Results Window — vanilla JS against the Tauri global API.
// Events from the translation loop: "loop-event" (translated | error) and
// "engine-state" (status snapshot with the capture target, zone list,
// Excluded Blocks + hotkeys). Zone/exclusion management (rename/delete/
// clear) and mode switching invoke the shell directly; config editing lives
// in the Settings window (ticket 06). No build step.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { writeText } = window.__TAURI__.clipboardManager;

const statusPill = document.getElementById("status-pill");
const privacyChip = document.getElementById("privacy-chip");
const adjustChip = document.getElementById("adjust-chip");
const toggleBtn = document.getElementById("toggle-btn");
const pauseBtn = document.getElementById("pause-btn");
const adjustBtn = document.getElementById("adjust-btn");
const settingsBtn = document.getElementById("settings-btn");
const configLine = document.getElementById("config-line");
const warningsLine = document.getElementById("warnings-line");
const errorBanner = document.getElementById("error-banner");
const resultsList = document.getElementById("results");
const emptyState = document.getElementById("empty-state");
const emptyHint = document.getElementById("empty-hint");
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

let running = false;
let paused = false;
let zones = [];
let excludedBlocks = [];
let monitors = [];
let mode = "zones";
let fullscreenMonitor = "";
let hotkeys = {
  toggle: "Alt+T",
  newZone: "Alt+Z",
  pause: "Alt+P",
  adjust: "Alt+A",
  oneShot: "Alt+Q",
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
    statusPill.textContent = "Config error";
    statusPill.className = "pill error";
  } else if (running && paused) {
    statusPill.textContent = "Paused";
    statusPill.className = "pill paused";
  } else if (running) {
    statusPill.textContent = "Running";
    statusPill.className = "pill running";
  } else {
    statusPill.textContent = "Stopped";
    statusPill.className = "pill stopped";
  }
  adjustChip.classList.toggle("hidden", !status.adjustMode);
  adjustBtn.textContent = status.adjustMode ? "Exit Adjust" : "Adjust";

  // Privacy badge (ticket 06): shown whenever the active Translate Engine
  // sends screen text off-machine (same data as the tray tooltip).
  const privacyEngine = status.privacyEngine;
  privacyChip.classList.toggle("hidden", !privacyEngine);
  if (privacyEngine) {
    privacyChip.title = `${privacyEngine} sends screen text to the cloud while translation runs`;
  }

  toggleBtn.textContent = running ? "Stop" : "Start";
  toggleBtn.classList.toggle("primary", !running);
  pauseBtn.disabled = !running;
  pauseBtn.textContent = paused ? "Resume" : "Pause";

  const watched =
    mode === "fullscreen"
      ? "full-screen mode"
      : `${zones.filter((z) => z.enabled).length} zone${zones.length === 1 ? "" : "s"}`;
  // Ticket 07: when a Profile matches the foreground app, its settings are
  // what the loop runs with — name it so the line tells the truth.
  const activeProfile = (status.profiles || []).find(
    (p) => p.id && p.id === status.activeProfileId,
  );
  const profileNote = activeProfile ? ` · profile "${activeProfile.name}"` : "";
  configLine.textContent = status.configError
    ? `Config: ${status.configPath}`
    : `${status.language} → ${status.targetLanguage}${profileNote} · ${watched} · every ${status.cadenceMs}ms · hold ${status.stabilityMs}ms · glossary ${status.glossaryTerms} terms · ${status.configPath}`;

  if (status.warnings && status.warnings.length > 0) {
    warningsLine.textContent = status.warnings.join("\n");
    warningsLine.classList.remove("hidden");
  } else {
    warningsLine.classList.add("hidden");
  }

  emptyHint.textContent = `Draw a Capture Zone with ${hotkeys.newZone}, or run a One-shot Capture with ${hotkeys.oneShot}. Translation toggles with ${hotkeys.toggle}, pauses with ${hotkeys.pause}, Adjust Mode with ${hotkeys.adjust}.`;

  renderModeControls();
  renderZones();
  renderExcluded();
  renderErrorBanner();
}

// ---- Capture Target (zones vs full-screen + monitor) ----

function renderModeControls() {
  modeZones.checked = mode === "zones";
  modeFullscreen.checked = mode === "fullscreen";
  monitorRow.classList.toggle("hidden", mode !== "fullscreen");
  modeHint.textContent =
    mode === "fullscreen"
      ? "watching every block on the selected monitor"
      : "watching the Capture Zones below";
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
    option.textContent = "no monitors detected";
    option.disabled = true;
    monitorSelect.append(option);
    return;
  }
  for (const m of monitors) {
    const option = document.createElement("option");
    option.value = m.name;
    option.textContent = m.primary ? `${m.name} (primary)` : m.name;
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
  excludedTitle.textContent = `Excluded Blocks (${excludedBlocks.length})`;

  excludedList.replaceChildren();
  for (const exclusion of excludedBlocks) {
    excludedList.append(excludedRow(exclusion));
  }

  if (excludedBlocks.length === 0) {
    excludedEmpty.textContent =
      "Nothing excluded yet — switch to full-screen mode, press Adjust (Alt+A), and click a translation to hide that position permanently.";
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
  label.textContent = exclusion.label || "(unlabeled block)";
  label.title = `Position ${exclusion.region}${exclusion.monitor ? ` on ${exclusion.monitor}` : ""}`;

  const meta = document.createElement("span");
  meta.className = "zone-region";
  meta.textContent = exclusion.region;

  const del = document.createElement("button");
  del.className = "zone-delete";
  del.title = "Clear this exclusion (the block translates again)";
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
  statusPill.textContent = "Error";
  statusPill.className = "pill error";
  renderErrorBanner();
}

function renderErrorBanner() {
  if (lastError) {
    errorBanner.textContent = `[${lastError.stage}] ${lastError.message}`;
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
      ? `Capture Zones (${zones.length} — not watched in full-screen mode)`
      : `Capture Zones (${enabledCount}${zones.length !== enabledCount ? ` of ${zones.length}` : ""})`;

  zonesList.replaceChildren();
  for (const zone of zones) {
    zonesList.append(zoneRow(zone));
  }

  if (zones.length === 0) {
    zonesEmpty.textContent = `Draw a Capture Zone with ${hotkeys.newZone} (or press "+ New Zone") and drag a rectangle over some text.`;
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
  name.title = "Click to rename";
  name.textContent = zone.enabled ? zone.name : `${zone.name} (monitor unplugged)`;
  name.addEventListener("click", () => beginRename(li, zone, name));

  const region = document.createElement("span");
  region.className = "zone-region";
  region.textContent = zone.monitor ? `${zone.region} · ${zone.monitor}` : zone.region;

  const del = document.createElement("button");
  del.className = "zone-delete";
  del.title = `Delete “${zone.name}”`;
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
  // the loop is stopped — that is the point of Alt+Q.

  emptyState.classList.add("hidden");

  const li = document.createElement("li");
  li.className = "entry";
  li.title = "Click to copy the Thai translation";

  const meta = document.createElement("div");
  meta.className = "meta";
  const badge = document.createElement("span");
  if (origin && origin.kind === "one_shot") {
    badge.className = "badge oneshot";
    badge.textContent = "one-shot";
  } else {
    badge.className = "badge zone";
    badge.textContent = (origin && origin.name) || "zone";
  }
  const time = document.createElement("span");
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
  renderStatus(await invoke("get_status"));

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
