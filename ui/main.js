// Langception Results Window — vanilla JS against the Tauri global API.
// Events from the translation loop: "loop-event" (translated | error) and
// "engine-state" (status snapshot with the zone list + hotkeys). Zone
// management (rename/delete) invokes the shell directly. No build step.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { writeText } = window.__TAURI__.clipboardManager;

const statusPill = document.getElementById("status-pill");
const toggleBtn = document.getElementById("toggle-btn");
const pauseBtn = document.getElementById("pause-btn");
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

let running = false;
let paused = false;
let zones = [];
let hotkeys = { toggle: "Alt+T", newZone: "Alt+Z", pause: "Alt+P", oneShot: "Alt+Q" };
let lastError = null; // { stage, message } — stays visible until the next success

function renderStatus(status) {
  running = status.running;
  paused = status.paused;
  zones = status.zones || [];
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

  toggleBtn.textContent = running ? "Stop" : "Start";
  toggleBtn.classList.toggle("primary", !running);
  pauseBtn.disabled = !running;
  pauseBtn.textContent = paused ? "Resume" : "Pause";

  configLine.textContent = status.configError
    ? `Config: ${status.configPath}`
    : `${status.language} → ${status.targetLanguage} · ${zones.length} zone${zones.length === 1 ? "" : "s"} · every ${status.cadenceMs}ms · glossary ${status.glossaryTerms} terms · ${status.configPath}`;

  if (status.warnings && status.warnings.length > 0) {
    warningsLine.textContent = status.warnings.join("\n");
    warningsLine.classList.remove("hidden");
  } else {
    warningsLine.classList.add("hidden");
  }

  emptyHint.textContent = `Draw a Capture Zone with ${hotkeys.newZone}, or run a One-shot Capture with ${hotkeys.oneShot}. Translation toggles with ${hotkeys.toggle}, pauses with ${hotkeys.pause}.`;

  renderZones();
  renderErrorBanner();
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
  zonesTitle.textContent = `Capture Zones (${zones.length})`;

  zonesList.replaceChildren();
  for (const zone of zones) {
    zonesList.append(zoneRow(zone));
  }

  if (zones.length === 0) {
    zonesEmpty.textContent = `No Capture Zones yet — press ${hotkeys.newZone} (or “+ New Zone”) and drag a rectangle over some text.`;
    zonesEmpty.classList.remove("hidden");
  } else {
    zonesEmpty.classList.add("hidden");
  }
}

function zoneRow(zone) {
  const li = document.createElement("li");
  li.className = "zone-row";

  const name = document.createElement("button");
  name.className = "zone-name";
  name.title = "Click to rename";
  name.textContent = zone.name;
  name.addEventListener("click", () => beginRename(li, zone, name));

  const region = document.createElement("span");
  region.className = "zone-region";
  region.textContent = zone.region;

  const del = document.createElement("button");
  del.className = "zone-delete";
  del.title = `Delete “${zone.name}”`;
  del.textContent = "✕";
  del.addEventListener("click", async () => {
    del.disabled = true;
    try {
      await invoke("delete_zone", { id: zone.id });
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
});
