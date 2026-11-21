// Langception Results Window — vanilla JS against the Tauri global API.
// Events from the translation loop: "loop-event" (translated | error) and
// "engine-state" (status snapshot). No build step.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { writeText } = window.__TAURI__.clipboardManager;

const statusPill = document.getElementById("status-pill");
const toggleBtn = document.getElementById("toggle-btn");
const configLine = document.getElementById("config-line");
const errorBanner = document.getElementById("error-banner");
const resultsList = document.getElementById("results");
const emptyState = document.getElementById("empty-state");
const copiedToast = document.getElementById("copied-toast");

let running = false;
let lastError = null; // { stage, message } — stays visible until the next success

function renderStatus(status) {
  running = status.running;

  if (status.configError) {
    statusPill.textContent = "Config error";
    statusPill.className = "pill error";
  } else if (running) {
    statusPill.textContent = "Running";
    statusPill.className = "pill running";
  } else {
    statusPill.textContent = "Stopped";
    statusPill.className = "pill stopped";
  }

  toggleBtn.textContent = running ? "Stop" : "Start";
  toggleBtn.classList.toggle("primary", !running);

  configLine.textContent = status.configError
    ? `Config: ${status.configPath}`
    : `${status.language} → ${status.targetLanguage} · region ${status.region} · every ${status.cadenceMs}ms · glossary ${status.glossaryTerms} terms · ${status.configPath}`;

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

function addEntry(block, translation, via, atMs) {
  lastError = null;
  renderErrorBanner();
  if (!running) return; // late event after stop; ignore

  emptyState.classList.add("hidden");

  const li = document.createElement("li");
  li.className = "entry";
  li.title = "Click to copy the Thai translation";

  const meta = document.createElement("div");
  meta.className = "meta";
  const badge = document.createElement("span");
  badge.className = `badge ${via}`;
  badge.textContent = via === "glossary" ? "glossary" : "engine";
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
      addEntry(payload.block, payload.translation, payload.via, payload.at_ms);
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
});
