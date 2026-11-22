// Langception Settings window — vanilla JS against the Tauri global API.
// One surface for every config: General, Engines (OCR pack detection),
// Glossary (full management + JSON import/export), Profiles (per-app
// configuration, ticket 07), Text Style, Hotkeys (rebinds apply live),
// About. Doubles as the first-run wizard host: when status.firstRun the
// wizard view shows before the tabs and is skippable. No build step.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open: openFileDialog, save: saveFileDialog } = window.__TAURI__.dialog;

// Tabs + pages ---------------------------------------------------------------

const wizard = document.getElementById("wizard");
const tabs = document.getElementById("tabs");
const pages = document.getElementById("tab-pages");
const toastEl = document.getElementById("toast");
const privacyChip = document.getElementById("privacy-chip");
const configErrorChip = document.getElementById("config-error-chip");

// Frameless window controls (min/close) are wired by titlebar.js, loaded
// before this script.

let toastTimer = null;
function toast(message, kind = "ok") {
  toastEl.textContent = message;
  toastEl.className = `toast ${kind}`;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => toastEl.classList.add("hidden"), 3500);
}

function showWizard() {
  wizard.classList.remove("hidden");
  tabs.classList.add("hidden");
  pages.classList.add("hidden");
  loadPacks("wizard");
  // The Azure upgrade step prefills from the saved engine settings.
  loadEngines();
}

function finishWizard() {
  // Skipping is allowed and changes nothing: zero-config works (the
  // keyless Google engine is already active).
  wizard.classList.add("hidden");
  tabs.classList.remove("hidden");
  pages.classList.remove("hidden");
}

let enginesPacksLoaded = false;
for (const tab of document.querySelectorAll(".tab")) {
  tab.addEventListener("click", () => {
    for (const t of document.querySelectorAll(".tab")) t.classList.toggle("active", t === tab);
    for (const page of document.querySelectorAll(".page")) {
      page.classList.toggle("active", page.id === `tab-${tab.dataset.tab}`);
    }
    // The Engines tab probes the OCR packs the first time it is opened.
    if (tab.dataset.tab === "engines" && !enginesPacksLoaded) {
      enginesPacksLoaded = true;
      loadPacks("engines");
    }
    // The Profiles tab scans the running apps the first time it is opened
    // (its app picker only offers apps that actually have a window).
    if (tab.dataset.tab === "profiles" && !runningAppsLoaded) {
      loadRunningApps(false);
    }
    // The Engines tab loads the engine settings the first time it is opened
    // (the wizard loads them at show time — its upgrade action merges with
    // the saved keys).
    if (tab.dataset.tab === "engines" && !enginesLoaded) {
      loadEngines();
    }
  });
}

// OCR language packs (wizard step 1 + Engines tab) ----------------------------

const PACK_INSTALL_HINT = (name) =>
  `missing — Windows Settings → Time & Language → Language & region → Add a language → ${name} (includes OCR)`;

function packRow(status) {
  const li = document.createElement("li");
  li.className = "pack-row";

  const name = document.createElement("span");
  name.className = "pack-name";
  name.textContent = status.name;

  const pill = document.createElement("span");
  if (status.installed) {
    pill.className = "pill ok";
    pill.textContent = "installed";
    const tag = document.createElement("span");
    tag.className = "pack-tag";
    tag.textContent = status.packTag;
    li.append(name, pill, tag);
  } else {
    pill.className = "pill err";
    pill.textContent = "missing";
    const instructions = document.createElement("span");
    instructions.className = "pack-instructions";
    instructions.textContent = PACK_INSTALL_HINT(status.name);
    li.append(name, pill, instructions);
  }
  return li;
}

async function loadPacks(prefix) {
  const list = document.getElementById(`${prefix}-packs`);
  const note = document.getElementById(`${prefix}-packs-note`);
  list.replaceChildren();
  note.textContent = "checking…";
  try {
    const packs = await invoke("get_ocr_pack_status");
    for (const pack of packs) list.append(packRow(pack));
    const missing = packs.filter((p) => !p.installed);
    note.textContent = missing.length
      ? `${missing.length} pack${missing.length === 1 ? "" : "s"} missing`
      : "all four source languages can be read";
  } catch (e) {
    note.textContent = "";
    toast(String(e), "error");
  }
}

function wirePacks(prefix) {
  document.getElementById(`${prefix}-recheck`).addEventListener("click", () => loadPacks(prefix));
  document
    .getElementById(`${prefix}-open-language-settings`)
    .addEventListener("click", () => invoke("open_language_pack_settings").catch((e) => toast(String(e), "error")));
}

wirePacks("wizard");
wirePacks("engines");

document.getElementById("wizard-finish").addEventListener("click", finishWizard);
document.getElementById("wizard-skip").addEventListener("click", finishWizard);

// Engines (tickets 09 + 10) -----------------------------------------------------
// The global default engine + every configurable engine's settings (they
// live in config.toml like every other setting). The wizard's Azure upgrade
// shares this loader so its action can merge with the already-saved values.

const AZURE_CREATE_KEY_URL =
  "https://portal.azure.com/#create/Microsoft.CognitiveServicesTextTranslation";
const DEEPL_API_URL = "https://www.deepl.com/en/pro-api";
const GEMINI_CREATE_KEY_URL = "https://aistudio.google.com/apikey";

const enginesDefault = document.getElementById("engines-default");
const enginesReading = document.getElementById("engines-reading");
const enginesReadingWarning = document.getElementById("engines-reading-warning");
const enginesAzureKey = document.getElementById("engines-azure-key");
const enginesAzureRegion = document.getElementById("engines-azure-region");
const enginesDeeplKey = document.getElementById("engines-deepl-key");
const enginesOpenAiBaseUrl = document.getElementById("engines-openai-base-url");
const enginesOpenAiKey = document.getElementById("engines-openai-key");
const enginesOpenAiModel = document.getElementById("engines-openai-model");
const enginesOpenAiTemperature = document.getElementById("engines-openai-temperature");
const enginesLmStudioBaseUrl = document.getElementById("engines-lmstudio-base-url");
const enginesLmStudioModel = document.getElementById("engines-lmstudio-model");
const enginesLmStudioTemperature = document.getElementById("engines-lmstudio-temperature");
const enginesLmStudioNote = document.getElementById("engines-lmstudio-note");
const enginesGeminiKey = document.getElementById("engines-gemini-key");
const enginesGeminiModel = document.getElementById("engines-gemini-model");
const enginesGeminiTemperature = document.getElementById("engines-gemini-temperature");
const enginesNote = document.getElementById("engines-note");
const wizardAzureKey = document.getElementById("wizard-azure-key");
const wizardAzureRegion = document.getElementById("wizard-azure-region");
const wizardAzureNote = document.getElementById("wizard-azure-note");

let enginesLoaded = false;
let savedEngines = null; // the last values confirmed by get_engines

// The LM Studio model dropdown's placeholder option (ticket 10: the list
// fills from the server's /models — no free-text model entry needed).
const LMSTUDIO_MODEL_PLACEHOLDER = "— check the server to list models —";

function renderLmStudioModels(models, chosen) {
  enginesLmStudioModel.replaceChildren();
  const placeholder = document.createElement("option");
  placeholder.value = "";
  placeholder.textContent = models.length
    ? LMSTUDIO_MODEL_PLACEHOLDER
    : "— no models loaded in LM Studio —";
  enginesLmStudioModel.append(placeholder);
  // The saved model stays selectable even when the server no longer serves
  // it (it may be offline right now) so the config never silently changes.
  for (const model of models) {
    const option = document.createElement("option");
    option.value = model;
    option.textContent = model;
    enginesLmStudioModel.append(option);
  }
  if (chosen && !models.includes(chosen)) {
    const option = document.createElement("option");
    option.value = chosen;
    option.textContent = `${chosen} (saved, not currently served)`;
    enginesLmStudioModel.append(option);
  }
  enginesLmStudioModel.value = chosen || "";
}

async function loadEngines() {
  try {
    savedEngines = await invoke("get_engines");
    enginesLoaded = true;
    enginesDefault.value = savedEngines.defaultEngine;
    enginesReading.value = savedEngines.reading || "ocr";
    enginesReadingWarning.classList.toggle("hidden", enginesReading.value !== "fused");
    enginesAzureKey.value = savedEngines.azure.key;
    enginesAzureRegion.value = savedEngines.azure.region;
    enginesDeeplKey.value = savedEngines.deepl.key;
    enginesOpenAiBaseUrl.value = savedEngines.openai.baseUrl;
    enginesOpenAiKey.value = savedEngines.openai.apiKey;
    enginesOpenAiModel.value = savedEngines.openai.model;
    enginesOpenAiTemperature.value = savedEngines.openai.temperature;
    enginesLmStudioBaseUrl.value = savedEngines.lmstudio.baseUrl;
    enginesLmStudioTemperature.value = savedEngines.lmstudio.temperature;
    renderLmStudioModels([], savedEngines.lmstudio.model);
    enginesGeminiKey.value = savedEngines.gemini.apiKey;
    enginesGeminiModel.value = savedEngines.gemini.model;
    enginesGeminiTemperature.value = savedEngines.gemini.temperature;
    // The wizard's fields prefill too, so a half-finished setup survives
    // re-running the wizard.
    wizardAzureKey.value = savedEngines.azure.key;
    wizardAzureRegion.value = savedEngines.azure.region;
  } catch (e) {
    toast(String(e), "error");
  }
}

function collectEngines(defaultEngine) {
  return {
    defaultEngine,
    reading: enginesReading.value,
    azure: {
      key: enginesAzureKey.value.trim(),
      region: enginesAzureRegion.value.trim(),
    },
    deepl: {
      key: enginesDeeplKey.value.trim(),
    },
    openai: {
      baseUrl: enginesOpenAiBaseUrl.value.trim(),
      apiKey: enginesOpenAiKey.value.trim(),
      model: enginesOpenAiModel.value.trim(),
      temperature: Number(enginesOpenAiTemperature.value) || 0,
    },
    lmstudio: {
      baseUrl: enginesLmStudioBaseUrl.value.trim(),
      model: enginesLmStudioModel.value.trim(),
      temperature: Number(enginesLmStudioTemperature.value) || 0,
    },
    gemini: {
      apiKey: enginesGeminiKey.value.trim(),
      model: enginesGeminiModel.value.trim(),
      temperature: Number(enginesGeminiTemperature.value) || 0,
    },
  };
}

document.getElementById("engines-save").addEventListener("click", async () => {
  try {
    await invoke("set_engines", { engines: collectEngines(enginesDefault.value) });
    toast("Engines saved");
  } catch (e) {
    toast(String(e), "error");
  }
});

// The vision-reading latency/cost warning (ticket 12) follows the select
// live — the user sees what "fused" implies before saving it.
enginesReading.addEventListener("change", () => {
  enginesReadingWarning.classList.toggle("hidden", enginesReading.value !== "fused");
});

// LM Studio server check (ticket 10): one round-trip answers reachability
// AND fills the model dropdown from /models. Runs against the URL as typed
// — you can validate a fix before saving it.
document.getElementById("engines-lmstudio-check").addEventListener("click", async () => {
  const btn = document.getElementById("engines-lmstudio-check");
  btn.disabled = true;
  enginesLmStudioNote.textContent = "checking…";
  try {
    const result = await invoke("check_lmstudio", { baseUrl: enginesLmStudioBaseUrl.value.trim() });
    renderLmStudioModels(result.models, enginesLmStudioModel.value.trim());
    enginesLmStudioNote.textContent = result.message;
    if (result.reachable) {
      toast(result.message);
    } else {
      toast(result.message, "error");
    }
  } catch (e) {
    enginesLmStudioNote.textContent = "";
    toast(String(e), "error");
  } finally {
    btn.disabled = false;
  }
});

document.getElementById("engines-test").addEventListener("click", async () => {
  const btn = document.getElementById("engines-test");
  btn.disabled = true;
  enginesNote.textContent = "testing…";
  try {
    const thai = await invoke("test_engine", { engine: enginesDefault.value });
    enginesNote.textContent = `works — "Hello" → ${thai}`;
  } catch (e) {
    enginesNote.textContent = "";
    toast(String(e), "error");
  } finally {
    btn.disabled = false;
  }
});

for (const [buttonId, url] of [
  ["engines-azure-link", AZURE_CREATE_KEY_URL],
  ["wizard-azure-link", AZURE_CREATE_KEY_URL],
  ["engines-deepl-link", DEEPL_API_URL],
  ["engines-gemini-link", GEMINI_CREATE_KEY_URL],
]) {
  document.getElementById(buttonId).addEventListener("click", () => {
    invoke("open_engine_link", { url }).catch((e) => toast(String(e), "error"));
  });
}

// The wizard's "Use Azure F0": saves the Azure key from the wizard fields
// and switches the default engine in one go. The other engines' saved
// values are passed through untouched.
document.getElementById("wizard-azure-use").addEventListener("click", async () => {
  const key = wizardAzureKey.value.trim();
  if (!key) {
    toast("Paste an Azure API key first — create one with the link", "error");
    return;
  }
  try {
    await invoke("set_engines", {
      engines: {
        defaultEngine: "azure",
        reading: savedEngines?.reading || "ocr",
        azure: { key, region: wizardAzureRegion.value.trim() },
        deepl: { key: savedEngines?.deepl?.key || "" },
        openai: savedEngines?.openai || { baseUrl: "", apiKey: "", model: "", temperature: 0.2 },
        lmstudio: savedEngines?.lmstudio || { baseUrl: "", model: "", temperature: 0.2 },
        gemini: savedEngines?.gemini || { apiKey: "", model: "", temperature: 0.2 },
      },
    });
    await loadEngines();
    wizardAzureNote.textContent = "Azure F0 is now the default engine";
    toast("Azure F0 is now the default engine");
  } catch (e) {
    toast(String(e), "error");
  }
});

// General -------------------------------------------------------------------

const generalLanguage = document.getElementById("general-language");
const generalCadence = document.getElementById("general-cadence");
const generalStability = document.getElementById("general-stability");
const generalGenderless = document.getElementById("general-genderless");
const generalAutostart = document.getElementById("general-autostart");
// The window on-top toggles (ticket 34): saved with General, applied to
// the live windows immediately.
const generalResultsOnTop = document.getElementById("general-results-on-top");
const generalSettingsOnTop = document.getElementById("general-settings-on-top");
let generalLoaded = false;

function loadGeneral(status) {
  if (generalLoaded) return;
  generalLoaded = true;
  generalLanguage.value = status.language;
  generalCadence.value = status.cadenceMs;
  generalStability.value = status.stabilityMs;
  generalGenderless.checked = Boolean(status.genderless);
  generalAutostart.checked = Boolean(status.startWithWindows);
  generalResultsOnTop.checked = Boolean(status.resultsOnTop);
  generalSettingsOnTop.checked = Boolean(status.settingsOnTop);
}

document.getElementById("general-save").addEventListener("click", async () => {
  const cadence = Math.round(Number(generalCadence.value) || 500);
  const stability = Math.round(Number(generalStability.value) || 0);
  try {
    await invoke("set_general", {
      settings: {
        language: generalLanguage.value,
        cadenceMs: cadence,
        stabilityMs: stability,
        genderless: generalGenderless.checked,
        resultsOnTop: generalResultsOnTop.checked,
        settingsOnTop: generalSettingsOnTop.checked,
      },
    });
    toast("General settings saved");
  } catch (e) {
    toast(String(e), "error");
  }
});

generalAutostart.addEventListener("change", async () => {
  const wanted = generalAutostart.checked;
  generalAutostart.disabled = true;
  try {
    const now = await invoke("set_start_with_windows", { enabled: wanted });
    generalAutostart.checked = now;
    toast(now ? "Langception starts with Windows" : "Start-with-Windows disabled");
  } catch (e) {
    generalAutostart.checked = !wanted; // revert
    toast(String(e), "error");
  } finally {
    generalAutostart.disabled = false;
  }
});

// Glossary -------------------------------------------------------------------

const glossaryList = document.getElementById("glossary-list");
const glossaryEmpty = document.getElementById("glossary-empty");
const glossaryNote = document.getElementById("glossary-note");
const newTermInput = document.getElementById("glossary-new-term");
const newThaiInput = document.getElementById("glossary-new-thai");

async function renderGlossary() {
  let entries;
  try {
    entries = await invoke("list_glossary");
  } catch (e) {
    glossaryNote.textContent = "";
    toast(String(e), "error");
    return;
  }
  glossaryList.replaceChildren();
  for (const entry of entries) glossaryList.append(glossaryRow(entry));
  glossaryEmpty.classList.toggle("hidden", entries.length > 0);
  glossaryNote.textContent = `${entries.length} term${entries.length === 1 ? "" : "s"}`;
}

function glossaryRow(entry) {
  const li = document.createElement("li");
  li.className = "glossary-row";
  li.dataset.originalTerm = entry.term;

  const term = document.createElement("input");
  term.type = "text";
  term.className = "term";
  term.value = entry.term;
  term.spellcheck = false;

  const thai = document.createElement("input");
  thai.type = "text";
  thai.className = "thai";
  thai.value = entry.thai;
  thai.spellcheck = false;

  const del = document.createElement("button");
  del.className = "delete-btn";
  del.title = `Delete “${entry.term}”`;
  del.textContent = "✕";
  del.addEventListener("click", async () => {
    del.disabled = true;
    try {
      await invoke("delete_glossary_entry", { term: entry.term });
      renderGlossary();
    } catch (e) {
      del.disabled = false;
      toast(String(e), "error");
    }
  });

  // Inline edit: commit on Enter or blur when something changed. One commit
  // per row: a successful edit re-renders (replacing this row), and the
  // blur that replacement can fire on the detached inputs must not send a
  // second, now-stale update.
  let committed = false;
  const commit = async () => {
    if (committed) return;
    const newTerm = term.value.trim();
    const newThai = thai.value.trim();
    if (newTerm === entry.term && newThai === entry.thai) return;
    committed = true;
    try {
      await invoke("update_glossary_entry", {
        term: entry.term,
        newTerm,
        thai: newThai,
      });
      renderGlossary();
    } catch (e) {
      toast(String(e), "error");
      renderGlossary(); // revert to the stored values
    }
  };
  for (const input of [term, thai]) {
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") input.blur();
      if (e.key === "Escape") {
        input.value = input === term ? entry.term : entry.thai;
        input.blur();
      }
    });
    input.addEventListener("blur", () => {
      // One commit per row: the second blur (already consistent) no-ops.
      commit();
    });
  }

  li.append(term, thai, del);
  return li;
}

async function addGlossaryEntry() {
  const term = newTermInput.value;
  const thai = newThaiInput.value;
  try {
    await invoke("add_glossary_entry", { term, thai });
    newTermInput.value = "";
    newThaiInput.value = "";
    renderGlossary();
  } catch (e) {
    toast(String(e), "error");
  }
}

document.getElementById("glossary-add-btn").addEventListener("click", addGlossaryEntry);
for (const input of [newTermInput, newThaiInput]) {
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") addGlossaryEntry();
  });
}

document.getElementById("glossary-export").addEventListener("click", async () => {
  try {
    const path = await saveFileDialog({
      title: "Export glossary as JSON",
      defaultPath: "langception-glossary.json",
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!path) return;
    const count = await invoke("export_glossary", { path });
    toast(`Exported ${count} term${count === 1 ? "" : "s"}`);
  } catch (e) {
    toast(String(e), "error");
  }
});

document.getElementById("glossary-import").addEventListener("click", async () => {
  try {
    const path = await openFileDialog({
      title: "Import glossary from JSON",
      multiple: false,
      directory: false,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!path) return;
    const mode = document.getElementById("glossary-import-mode").value;
    const report = await invoke("import_glossary", { path, mode });
    toast(`Imported: ${report.added} added, ${report.updated} updated`);
    renderGlossary();
  } catch (e) {
    toast(String(e), "error");
  }
});

// Profiles (ticket 07) ---------------------------------------------------------

const profileSelect = document.getElementById("profile-select");
const profileNewBtn = document.getElementById("profile-new");
const profileDeleteBtn = document.getElementById("profile-delete");
const profileActiveChip = document.getElementById("profile-active-chip");
const profileEmpty = document.getElementById("profile-empty");
const profileEditor = document.getElementById("profile-editor");
const profileName = document.getElementById("profile-name");
const profileExe = document.getElementById("profile-exe");
const profileLanguage = document.getElementById("profile-language");
const profileEngine = document.getElementById("profile-engine");
const profileGenderless = document.getElementById("profile-genderless");
const profileGlossaryList = document.getElementById("profile-glossary-list");
const profileGlossaryEmpty = document.getElementById("profile-glossary-empty");
const profileGlossaryNewTerm = document.getElementById("profile-glossary-new-term");
const profileGlossaryNewThai = document.getElementById("profile-glossary-new-thai");
const profileZonesList = document.getElementById("profile-zones-list");
const profileZonesEmpty = document.getElementById("profile-zones-empty");

let profiles = [];
let selectedProfileId = null;
let populatedProfileId = null; // the id the editor inputs were last filled from
let runningApps = [];
let runningAppsLoaded = false;
let deleteArmedTimer = null;

async function loadRunningApps(force = false) {
  if (!force && runningAppsLoaded) return;
  try {
    runningApps = await invoke("list_running_apps");
    runningAppsLoaded = true;
  } catch (e) {
    toast(String(e), "error");
    runningApps = [];
  }
  renderExeOptions();
}

function renderExeOptions() {
  const chosen = profileExe.dataset.chosen || "";
  profileExe.replaceChildren();
  if (runningApps.length === 0) {
    const option = document.createElement("option");
    option.value = chosen;
    option.textContent = chosen || "no running apps — start the app, then ↻";
    profileExe.append(option);
    return;
  }
  for (const app of runningApps) {
    const option = document.createElement("option");
    option.value = app.exe;
    option.textContent = app.title ? `${app.title} — ${app.exe}` : app.exe;
    profileExe.append(option);
  }
  const match = runningApps.find((a) => a.exe.toLowerCase() === chosen.toLowerCase());
  profileExe.value = match ? match.exe : runningApps[0].exe;
}

function renderProfiles(status) {
  profiles = status.profiles || [];
  if (selectedProfileId && !profiles.some((p) => p.id === selectedProfileId)) {
    selectedProfileId = null;
  }
  if (!selectedProfileId && profiles.length > 0) selectedProfileId = profiles[0].id;

  // Rebuild the dropdown only when the option set changed, so a status
  // push never slams shut a dropdown the user has open (same rule as the
  // Results Window's monitor select).
  const signature = profiles
    .map((p) => `${p.id}\u0001${p.name}\u0001${p.active ? "a" : ""}`)
    .join("\u0000");
  if (profileSelect.dataset.sig !== signature) {
    profileSelect.dataset.sig = signature;
    profileSelect.replaceChildren();
    if (profiles.length === 0) {
      const option = document.createElement("option");
      option.textContent = "no profiles";
      option.value = "";
      option.disabled = true;
      profileSelect.append(option);
    }
    for (const profile of profiles) {
      const option = document.createElement("option");
      option.value = profile.id;
      option.textContent = profile.active ? `${profile.name} — active` : profile.name;
      profileSelect.append(option);
    }
  }
  profileSelect.value = selectedProfileId || "";

  const has = profiles.length > 0;
  profileEmpty.classList.toggle("hidden", has);
  profileEditor.classList.toggle("hidden", !has);
  profileDeleteBtn.disabled = !has;

  const selected = profiles.find((p) => p.id === selectedProfileId);
  profileActiveChip.classList.toggle("hidden", !selected?.active);
  renderProfileZones(selected);
  // Editor inputs only refill when the selection changes (or on explicit
  // save/create) — a status event must never clobber in-progress edits.
  if (selected && populatedProfileId !== selected.id) populateEditor(selected);
  if (!selected) populatedProfileId = null;
}

function populateEditor(profile) {
  populatedProfileId = profile.id;
  profileName.value = profile.name;
  profileExe.dataset.chosen = profile.exe;
  renderExeOptions();
  profileLanguage.value = profile.language;
  profileEngine.value = profile.engine || "";
  profileGenderless.value = profile.genderless || "inherit";
}

profileSelect.addEventListener("change", async () => {
  selectedProfileId = profileSelect.value || null;
  const selected = profiles.find((p) => p.id === selectedProfileId);
  profileActiveChip.classList.toggle("hidden", !selected?.active);
  if (selected) populateEditor(selected);
  renderProfileZones(selected);
  await renderProfileGlossary();
});

profileExe.addEventListener("change", () => {
  profileExe.dataset.chosen = profileExe.value;
  const app = runningApps.find((a) => a.exe === profileExe.value);
  // A fresh profile drafts its name from the app; saving renames freely.
  if (app && profileName.dataset.drafted !== "1") {
    profileName.value = uniqueProfileName(app.title || app.exe);
  }
});
profileName.addEventListener("input", () => {
  profileName.dataset.drafted = "1";
});

function uniqueProfileName(base) {
  const trimmed = (base || "").trim().slice(0, 40) || "New profile";
  let name = trimmed;
  for (let n = 2; profiles.some((p) => p.name.toLowerCase() === name.toLowerCase()); n += 1) {
    name = `${trimmed} ${n}`;
  }
  return name;
}

profileNewBtn.addEventListener("click", async () => {
  await loadRunningApps(true);
  if (runningApps.length === 0) {
    toast("No running apps found — start the app you want to profile, then try again", "error");
    return;
  }
  const first = runningApps[0];
  try {
    const id = await invoke("create_profile", {
      profile: {
        name: uniqueProfileName(first.title || first.exe),
        exe: first.exe,
        language: document.getElementById("general-language").value,
        engine: null,
        genderless: "inherit",
      },
    });
    selectedProfileId = id;
    renderProfiles(await invoke("get_status"));
    await renderProfileGlossary();
    toast("Profile created — adjust it and press Save profile");
  } catch (e) {
    toast(String(e), "error");
  }
});

// Two-click delete (no native confirm dialogs in the webview): the first
// click arms the button, the second deletes, idling disarms it.
profileDeleteBtn.addEventListener("click", async () => {
  if (!selectedProfileId) return;
  if (profileDeleteBtn.dataset.armed !== "1") {
    profileDeleteBtn.dataset.armed = "1";
    profileDeleteBtn.textContent = "Really delete?";
    clearTimeout(deleteArmedTimer);
    deleteArmedTimer = setTimeout(disarmDelete, 3000);
    return;
  }
  disarmDelete();
  const id = selectedProfileId;
  try {
    await invoke("delete_profile", { id });
    selectedProfileId = null;
    renderProfiles(await invoke("get_status"));
    toast("Profile deleted");
  } catch (e) {
    toast(String(e), "error");
  }
});

function disarmDelete() {
  clearTimeout(deleteArmedTimer);
  profileDeleteBtn.dataset.armed = "";
  profileDeleteBtn.textContent = "Delete";
}

document.getElementById("profile-exe-refresh").addEventListener("click", () => loadRunningApps(true));

document.getElementById("profile-save").addEventListener("click", async () => {
  if (!selectedProfileId) return;
  try {
    await invoke("update_profile", {
      id: selectedProfileId,
      profile: {
        name: profileName.value,
        exe: profileExe.value,
        language: profileLanguage.value,
        engine: profileEngine.value || null,
        genderless: profileGenderless.value,
      },
    });
    toast("Profile saved");
    renderProfiles(await invoke("get_status"));
    await renderProfileGlossary();
  } catch (e) {
    toast(String(e), "error");
  }
});

// Profile glossary layer — the global glossary's row UI, scoped to the
// selected profile and the profile-scoped commands.

async function renderProfileGlossary() {
  profileGlossaryList.replaceChildren();
  if (!selectedProfileId) {
    profileGlossaryEmpty.classList.add("hidden");
    return;
  }
  let entries;
  try {
    entries = await invoke("list_profile_glossary", { profileId: selectedProfileId });
  } catch (e) {
    toast(String(e), "error");
    return;
  }
  for (const entry of entries) profileGlossaryList.append(profileGlossaryRow(entry));
  profileGlossaryEmpty.classList.toggle("hidden", entries.length > 0);
}

function profileGlossaryRow(entry) {
  const li = document.createElement("li");
  li.className = "glossary-row";
  li.dataset.originalTerm = entry.term;

  const term = document.createElement("input");
  term.type = "text";
  term.className = "term";
  term.value = entry.term;
  term.spellcheck = false;

  const thai = document.createElement("input");
  thai.type = "text";
  thai.className = "thai";
  thai.value = entry.thai;
  thai.spellcheck = false;

  const del = document.createElement("button");
  del.className = "delete-btn";
  del.title = `Delete “${entry.term}”`;
  del.textContent = "✕";
  del.addEventListener("click", async () => {
    del.disabled = true;
    try {
      await invoke("delete_profile_glossary_entry", { profileId: selectedProfileId, term: entry.term });
      renderProfileGlossary();
    } catch (e) {
      del.disabled = false;
      toast(String(e), "error");
    }
  });

  // Same commit-once rule as the global glossary rows: the re-render after
  // a successful edit replaces this row, and its blur must not re-commit.
  let committed = false;
  const commit = async () => {
    if (committed) return;
    const newTerm = term.value.trim();
    const newThai = thai.value.trim();
    if (newTerm === entry.term && newThai === entry.thai) return;
    committed = true;
    try {
      await invoke("update_profile_glossary_entry", {
        profileId: selectedProfileId,
        term: entry.term,
        newTerm,
        thai: newThai,
      });
      renderProfileGlossary();
    } catch (e) {
      toast(String(e), "error");
      renderProfileGlossary(); // revert to the stored values
    }
  };
  for (const input of [term, thai]) {
    input.addEventListener("keydown", (e) => {
      if (e.key === "Enter") input.blur();
      if (e.key === "Escape") {
        input.value = input === term ? entry.term : entry.thai;
        input.blur();
      }
    });
    input.addEventListener("blur", () => commit());
  }

  li.append(term, thai, del);
  return li;
}

async function addProfileGlossaryEntry() {
  if (!selectedProfileId) return;
  try {
    await invoke("add_profile_glossary_entry", {
      profileId: selectedProfileId,
      term: profileGlossaryNewTerm.value,
      thai: profileGlossaryNewThai.value,
    });
    profileGlossaryNewTerm.value = "";
    profileGlossaryNewThai.value = "";
    renderProfileGlossary();
  } catch (e) {
    toast(String(e), "error");
  }
}

document.getElementById("profile-glossary-add-btn").addEventListener("click", addProfileGlossaryEntry);
for (const input of [profileGlossaryNewTerm, profileGlossaryNewThai]) {
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") addProfileGlossaryEntry();
  });
}

function renderProfileZones(profile) {
  profileZonesList.replaceChildren();
  const zones = profile?.zones || [];
  for (const zone of zones) profileZonesList.append(profileZoneRow(zone));
  profileZonesEmpty.classList.toggle("hidden", zones.length > 0);
  if (zones.length === 0) {
    profileZonesEmpty.textContent = selectedProfileId
      ? "No zones — the whole app window is watched."
      : "";
  }
}

function profileZoneRow(zone) {
  const li = document.createElement("li");
  li.className = "glossary-row";

  const name = document.createElement("span");
  name.className = "term";
  name.textContent = zone.name;

  const region = document.createElement("span");
  region.className = "hint";
  region.textContent = `${zone.region} (window-relative)`;

  const del = document.createElement("button");
  del.className = "delete-btn";
  del.title = `Delete “${zone.name}”`;
  del.textContent = "✕";
  del.addEventListener("click", async () => {
    del.disabled = true;
    try {
      await invoke("delete_profile_zone", { profileId: selectedProfileId, zoneId: zone.id });
      renderProfiles(await invoke("get_status"));
    } catch (e) {
      del.disabled = false;
      toast(String(e), "error");
    }
  });

  li.append(name, region, del);
  return li;
}

document.getElementById("profile-zone-draw").addEventListener("click", async () => {
  if (!selectedProfileId) return;
  try {
    await invoke("open_zone_picker", { mode: `profile:${selectedProfileId}` });
  } catch (e) {
    toast(String(e), "error");
  }
});

// Text Style -----------------------------------------------------------------

const styleFont = document.getElementById("style-font");
const styleSize = document.getElementById("style-size");
const styleAutoColors = document.getElementById("style-auto-colors");
const styleColor = document.getElementById("style-color");
const styleOutlineWidth = document.getElementById("style-outline-width");
const styleOutlineColor = document.getElementById("style-outline-color");
const styleBackground = document.getElementById("style-background");
const styleOpacity = document.getElementById("style-opacity");
const styleCoverSource = document.getElementById("style-cover-source");
const stylePlacement = document.getElementById("style-placement");
let styleLoaded = false;
let styleSaveTimer = null;

// `type=color` inputs need #rrggbb; config allows the short form.
function normalizeHex(value, fallback) {
  let v = (value || "").trim();
  if (!v.startsWith("#")) v = "#" + v;
  if (/^#[0-9a-fA-F]{3}$/.test(v)) {
    v = "#" + [...v.slice(1)].map((c) => c + c).join("");
  }
  return /^#[0-9a-fA-F]{6}$/.test(v) ? v.toLowerCase() : fallback;
}

function loadStyleInputs(style) {
  if (styleLoaded) return;
  styleLoaded = true;
  styleFont.value = style.fontFamily;
  styleSize.value = style.fontSize;
  styleAutoColors.checked = Boolean(style.autoColors);
  styleColor.disabled = styleAutoColors.checked;
  styleColor.value = normalizeHex(style.color, "#ffffff");
  styleOutlineWidth.value = style.outlineWidth;
  styleOutlineColor.value = normalizeHex(style.outlineColor, "#000000");
  styleBackground.checked = style.background;
  styleOpacity.value = style.backgroundOpacity;
  styleCoverSource.checked = style.coverSource;
  stylePlacement.value = style.placement || "over";
}

function collectStyle() {
  return {
    fontFamily: styleFont.value.trim() || "Noto Sans Thai",
    fontSize: Number(styleSize.value) || 16,
    autoColors: styleAutoColors.checked,
    color: normalizeHex(styleColor.value, "#ffffff"),
    outlineWidth: Number(styleOutlineWidth.value) || 0,
    outlineColor: normalizeHex(styleOutlineColor.value, "#000000"),
    background: styleBackground.checked,
    backgroundOpacity: Number(styleOpacity.value) || 0,
    coverSource: styleCoverSource.checked,
    placement: stylePlacement.value,
  };
}

// Exact live preview (ticket 26): collectStyle() is the single source of
// truth — the exact object a save would store — pushed through the
// overlay's shared pill renderer (pill-style.js), so the preview cannot
// drift from what the In-place Overlay draws. Geometry (Placement, Cover
// Source stretch) is out of scope: it depends on each block's source rect,
// which settings does not have. Auto Colors sampling needs capture data,
// so the manual colors — the documented fallback — always show; the note
// under the pill says so while Auto Colors is on.
const stylePreviewPill = document.getElementById("style-preview-pill");
const stylePreviewText = document.getElementById("style-preview-text");
const stylePreviewNote = document.getElementById("style-preview-note");

function updateStylePreview() {
  const style = collectStyle();
  applyPillStyle(stylePreviewPill, style, null);
  // The overlay's fitText always sets the size explicitly on the text
  // element; the preview does the same, at the configured size.
  stylePreviewText.style.fontSize = style.fontSize + "px";
  stylePreviewNote.classList.toggle("hidden", !style.autoColors);
}

// Sliders fire a burst of input events; a short trailing debounce keeps the
// live feel without hammering config.toml per pixel.
function scheduleStyleSave() {
  clearTimeout(styleSaveTimer);
  styleSaveTimer = setTimeout(async () => {
    try {
      await invoke("set_text_style", { style: collectStyle() });
    } catch (e) {
      toast(String(e), "error");
    }
  }, 250);
}

for (const control of [styleFont, styleSize, styleAutoColors, styleColor, styleOutlineWidth, styleOutlineColor, styleBackground, styleOpacity, styleCoverSource, stylePlacement]) {
  // The preview repaints instantly from the in-form values; the save to
  // config.toml stays debounced behind it.
  control.addEventListener("input", updateStylePreview);
  control.addEventListener("change", updateStylePreview);
  control.addEventListener("input", scheduleStyleSave);
  control.addEventListener("change", scheduleStyleSave);
}

// Auto Colors overrides the manual text color per block; grey the picker
// out while it is on so the panel reads truthfully about what applies.
styleAutoColors.addEventListener("change", () => {
  styleColor.disabled = styleAutoColors.checked;
});

// Font selector (ticket 15): the dropdown lists the fonts actually installed
// on this PC (DirectWrite), while the field stays typeable so a hand-edited
// config name never breaks. (Its font-only preview grew into the exact
// whole-style preview above — ticket 26.)
async function loadFontList() {
  if (styleFont.dataset.loaded) return;
  styleFont.dataset.loaded = "true";
  try {
    const fonts = await invoke("list_fonts");
    const list = document.getElementById("font-list");
    for (const font of fonts) {
      const option = document.createElement("option");
      option.value = font;
      list.append(option);
    }
  } catch (e) {
    styleFont.dataset.loaded = ""; // retry next open: the list is optional
    toast(`Could not list installed fonts: ${e}`, "error");
  }
}

// Renders the fallback style immediately; DOMContentLoaded refreshes it
// with the saved config values once the inputs are filled.
updateStylePreview();
loadFontList();

// Hotkeys --------------------------------------------------------------------

const hotkeyInputs = {
  toggle: document.getElementById("hotkey-toggle"),
  newZone: document.getElementById("hotkey-new-zone"),
  pause: document.getElementById("hotkey-pause"),
  adjust: document.getElementById("hotkey-adjust"),
  oneShot: document.getElementById("hotkey-one-shot"),
};
let hotkeysLoaded = false;

function loadHotkeys(hotkeys) {
  if (hotkeysLoaded) return;
  hotkeysLoaded = true;
  hotkeyInputs.toggle.value = hotkeys.toggle;
  hotkeyInputs.newZone.value = hotkeys.newZone;
  hotkeyInputs.pause.value = hotkeys.pause;
  hotkeyInputs.adjust.value = hotkeys.adjust;
  hotkeyInputs.oneShot.value = hotkeys.oneShot;
}

document.getElementById("hotkeys-save").addEventListener("click", async () => {
  try {
    await invoke("set_hotkeys", {
      hotkeys: {
        toggle: hotkeyInputs.toggle.value,
        newZone: hotkeyInputs.newZone.value,
        pause: hotkeyInputs.pause.value,
        adjust: hotkeyInputs.adjust.value,
        oneShot: hotkeyInputs.oneShot.value,
      },
    });
    toast("Hotkeys saved and re-registered");
  } catch (e) {
    toast(String(e), "error");
  }
});

// About ----------------------------------------------------------------------

async function loadAbout() {
  try {
    const about = await invoke("get_about");
    document.getElementById("about-product").textContent = about.productName;
    document.getElementById("about-version").textContent = about.version;
    document.getElementById("about-target").textContent = about.targetLanguage;
    document.getElementById("about-config-path").textContent = about.configPath;
    document.getElementById("about-logs-path").textContent = about.logsPath;
  } catch (e) {
    toast(String(e), "error");
  }
}

document.getElementById("about-open-config").addEventListener("click", () => {
  invoke("open_config_folder").catch((e) => toast(String(e), "error"));
});

document.getElementById("about-open-logs").addEventListener("click", () => {
  invoke("open_logs_folder").catch((e) => toast(String(e), "error"));
});

document.getElementById("about-wizard").addEventListener("click", showWizard);

// Boot -----------------------------------------------------------------------

window.addEventListener("DOMContentLoaded", async () => {
  let status;
  try {
    status = await invoke("get_status");
  } catch (e) {
    toast(String(e), "error");
    return;
  }

  loadGeneral(status);
  loadStyleInputs(status.textStyle);
  updateStylePreview();
  loadHotkeys(status.hotkeys);
  renderGlossary();
  renderProfiles(status);
  loadAbout();
  updateChips(status);

  if (status.firstRun) {
    showWizard();
  } else {
    finishWizard();
  }

  await listen("engine-state", (event) => {
    updateChips(event.payload);
    // The Profiles list carries the per-profile `active` flags, which move
    // with the foreground app — refresh on every status push.
    renderProfiles(event.payload);
  });
});

function updateChips(status) {
  // Privacy badge mirror (ticket 06): same data the Results Window badge
  // and the tray tooltip use.
  const cloud = status.privacyEngine;
  privacyChip.classList.toggle("hidden", !cloud);
  if (cloud) {
    privacyChip.title = `${cloud} sends screen text off-machine while translation runs`;
  }
  configErrorChip.classList.toggle("hidden", !status.configError);
}
