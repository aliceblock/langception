// Langception Settings window — vanilla JS against the Tauri global API.
// One surface for every config: General, Engines (OCR pack detection),
// Glossary (full management + JSON import/export), Profiles (per-app
// configuration, ticket 07), Text Style, Hotkeys (rebinds apply live),
// About. Doubles as the first-run wizard host: when status.firstRun the
// wizard view shows before the tabs and is skippable. No build step.
// Ticket 39: the per-tab Save buttons are gone — one sticky bottom-right
// Save footer shows only while some explicit-save field is dirty, and
// closing the window warns before discarding unsaved edits.
// Ticket 51: a rejected save reverts the tab's fields to that same
// snapshot — the form never keeps showing values the config refused.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open: openFileDialog, save: saveFileDialog, ask: askDialog } = window.__TAURI__.dialog;

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
    // (its app picker only offers apps that actually have a window), and
    // loads the preset list its System Prompt Preset selector offers.
    if (tab.dataset.tab === "profiles") {
      if (!runningAppsLoaded) loadRunningApps(false);
      if (!profilePresetsLoaded) {
        profilePresetsLoaded = true;
        loadPromptPresets().then(renderProfilePresetOptions).catch((e) => toast(String(e), "error"));
      }
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

const AZURE_CREATE_KEY_URL = "https://portal.azure.com";
const DEEPL_API_URL = "https://www.deepl.com/en/pro-api";
const GEMINI_CREATE_KEY_URL = "https://aistudio.google.com/apikey";

const enginesDefault = document.getElementById("engines-default");
const enginesOcr = document.getElementById("engines-ocr");
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
// The custom LLM prompt fields (ticket 37): empty = the built-in default.
const enginesPromptTranslation = document.getElementById("engines-prompt-translation");
const enginesPromptScreenReading = document.getElementById("engines-prompt-screen-reading");
// The System Prompt Preset selector and its save/rename/delete row (ticket
// 42): a preset loads its two texts into the two prompt fields.
const enginesPreset = document.getElementById("engines-preset");
const enginesPresetName = document.getElementById("engines-preset-name");
const enginesPresetSave = document.getElementById("engines-preset-save");
const enginesPresetRename = document.getElementById("engines-preset-rename");
const enginesPresetDelete = document.getElementById("engines-preset-delete");
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
    enginesOcr.value = savedEngines.ocrEngine || "windows";
    enginesReading.value = savedEngines.reading || "ocr";
    enginesReadingWarning.classList.toggle("hidden", enginesReading.value !== "fused");
    // Ticket 54: the saved engine's card is the expanded one.
    expandSelectedEngineCard();
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
    enginesPromptTranslation.value = savedEngines.translationPrompt || "";
    enginesPromptScreenReading.value = savedEngines.screenReadingPrompt || "";
    await loadPromptPresets();
    applyPromptSelection(savedEngines.promptPreset || GENERAL_PRESET);
    // The wizard's fields prefill too, so a half-finished setup survives
    // re-running the wizard.
    wizardAzureKey.value = savedEngines.azure.key;
    wizardAzureRegion.value = savedEngines.azure.region;
    // Ticket 39: these just-loaded values are the engines baseline (the tab
    // loads lazily, so the boot snapshot only covered the empty controls).
    snapshotFields(TRACKED_ENGINES);
    updateSaveFooter();
  } catch (e) {
    toast(String(e), "error");
  }
}

// System Prompt Presets (ticket 42). The two prompt fields stay exactly what
// gets sent: picking a preset loads its texts into them, and typing turns
// the selection into Custom. The Rust side decides what a save amounted to
// and reports the selection the config holds — an edited preset is stored
// as "custom", a deleted selected one as "general" — and the page shows
// that instead of guessing.
const CUSTOM_PRESET = "custom";
const GENERAL_PRESET = "general";
let promptPresets = []; // the last list confirmed by list_prompt_presets
let activePromptPreset = GENERAL_PRESET; // the selection the config holds
// The hand-written texts, remembered while a preset's wording is on show,
// so trying presets never loses them.
let customPrompts = { translation: "", screenReading: "" };

function promptFields() {
  return {
    translation: enginesPromptTranslation.value,
    screenReading: enginesPromptScreenReading.value,
  };
}

// Under the custom selection the fields ARE the hand-written texts.
function syncCustomPrompts(active) {
  if (active === CUSTOM_PRESET) customPrompts = promptFields();
}

function findPromptPreset(name) {
  return promptPresets.find((preset) => preset.name === name);
}

async function loadPromptPresets() {
  promptPresets = (await invoke("list_prompt_presets")) || [];
}

// Rebuilt only when the option set changed, so a refresh never slams shut a
// dropdown the user has open (same rule as the Profiles select).
function renderPresetOptions() {
  const signature = promptPresets.map((preset) => `${preset.name}\u0001${preset.builtin}`).join("\u0000");
  if (enginesPreset.dataset.sig === signature) return;
  enginesPreset.dataset.sig = signature;
  const builtins = document.createElement("optgroup");
  builtins.label = "Built-in presets";
  const saved = document.createElement("optgroup");
  saved.label = "Your presets";
  for (const preset of promptPresets) {
    const option = document.createElement("option");
    option.value = preset.name;
    option.textContent = preset.label;
    (preset.builtin ? builtins : saved).append(option);
  }
  const custom = document.createElement("option");
  custom.value = CUSTOM_PRESET;
  custom.textContent = "Custom — no preset";
  enginesPreset.replaceChildren(builtins);
  if (saved.childElementCount > 0) enginesPreset.append(saved);
  enginesPreset.append(custom);
  enginesPreset.value = activePromptPreset;
}

// Rename and Delete manage the STORED preset the dropdown shows — not
// merely the one in force (an unselected preset needs no detour through
// Save to be renamed or deleted). Built-ins are fixed, and General/Custom
// are not presets to manage.
function updatePresetControls() {
  const picked = findPromptPreset(enginesPreset.value);
  const manageable = Boolean(picked && !picked.builtin);
  enginesPresetRename.disabled = !manageable;
  enginesPresetDelete.disabled = !manageable;
}

// Reflects the selection the config holds. "general" means the Rust side
// cleared both prompts (a deleted selected preset) — mirror that; a named
// preset means its wording is in force — show it. Custom leaves the fields
// alone: they ARE the hand-written texts.
function showPromptPreset(active) {
  activePromptPreset = active;
  renderPresetOptions();
  enginesPreset.value = active;
  if (active === GENERAL_PRESET) {
    enginesPromptTranslation.value = "";
    enginesPromptScreenReading.value = "";
  } else if (active !== CUSTOM_PRESET) {
    const inForce = findPromptPreset(active);
    if (inForce) {
      enginesPromptTranslation.value = inForce.translation;
      enginesPromptScreenReading.value = inForce.screenReading;
    }
  }
  updatePresetControls();
}

// The one "the config's answer landed" step, shared by every preset save
// path: record the hand-written texts under Custom, then reflect.
function applyPromptSelection(active) {
  syncCustomPrompts(active);
  showPromptPreset(active);
}

// The preset commands store the two prompts as displayed: re-baseline only
// the prompt controls, never the whole Engines form — its other fields may
// hold unsaved edits the footer must keep asking about.
function promptControlsSaved() {
  snapshotFields([enginesPreset, enginesPromptTranslation, enginesPromptScreenReading]);
  updateSaveFooter();
}

async function confirmDialog(message) {
  try {
    return await askDialog(message, { title: "Langception — Settings", kind: "warning" });
  } catch (e) {
    // A failed dialog must not strand the action silently.
    toast(String(e), "error");
    return false;
  }
}

async function refreshAfterPromptPresetEdit(active, message) {
  await loadPromptPresets();
  showPromptPreset(active);
  // The Profiles tab's binding selector offers the same list, and a
  // rename/delete moves (or resets) the stored bindings (ticket 43).
  renderProfilePresetOptions();
  promptControlsSaved();
  toast(message);
}

enginesPreset.addEventListener("change", () => {
  if (enginesPreset.value === CUSTOM_PRESET) {
    // Back to the hand-written texts that were on show before a preset.
    enginesPromptTranslation.value = customPrompts.translation;
    enginesPromptScreenReading.value = customPrompts.screenReading;
  } else {
    const preset = findPromptPreset(enginesPreset.value);
    enginesPromptTranslation.value = preset ? preset.translation : "";
    enginesPromptScreenReading.value = preset ? preset.screenReading : "";
  }
  // The name box doubles as the Rename target, so under a saved preset it
  // holds that preset's name. Nothing else touches it: a name typed for a
  // new preset must survive picking a starting wording.
  const picked = findPromptPreset(enginesPreset.value);
  if (picked && !picked.builtin) enginesPresetName.value = picked.name;
  updatePresetControls();
});

for (const field of [enginesPromptTranslation, enginesPromptScreenReading]) {
  field.addEventListener("input", () => {
    // Typing leaves whatever preset was loaded: the texts are the user's.
    customPrompts = promptFields();
    enginesPreset.value = CUSTOM_PRESET;
    updatePresetControls();
  });
}

enginesPresetSave.addEventListener("click", async () => {
  const name = enginesPresetName.value.trim();
  if (!name) {
    toast("Type a name for the preset first", "error");
    enginesPresetName.focus();
    return;
  }
  const { translation, screenReading } = promptFields();
  if (!translation.trim() && !screenReading.trim()) {
    toast("Nothing to save — both prompts are empty, which is the General preset", "error");
    return;
  }
  // Saving over a saved preset replaces it, so ask first (the Rust side
  // refuses without the flag); a built-in's name is simply refused there.
  const existing = promptPresets.find(
    (preset) => !preset.builtin && preset.name.toLowerCase() === name.toLowerCase()
  );
  if (existing) {
    const replace = await confirmDialog(
      `A preset named "${existing.name}" already exists. Replace it with the prompts as they read now?`
    );
    if (!replace) return;
  }
  try {
    const active = await invoke("save_prompt_preset", {
      name,
      translation,
      screenReading,
      overwrite: Boolean(existing),
    });
    await refreshAfterPromptPresetEdit(active, `Preset "${active}" saved and selected`);
    // The new preset is the selection in force; its name stays as the
    // Rename target, the way the config spells it.
    enginesPresetName.value = active;
  } catch (e) {
    toast(String(e), "error");
  }
});
enginesPresetName.addEventListener("keydown", (e) => {
  if (e.key === "Enter") enginesPresetSave.click();
});

enginesPresetRename.addEventListener("click", async () => {
  const from = findPromptPreset(enginesPreset.value);
  if (!from || from.builtin) {
    toast("Pick one of your saved presets to rename", "error");
    return;
  }
  const to = enginesPresetName.value.trim();
  if (!to) {
    toast("Type the new name first", "error");
    enginesPresetName.focus();
    return;
  }
  if (to === from.name) {
    toast("That is already its name");
    return;
  }
  try {
    const active = await invoke("rename_prompt_preset", { name: from.name, newName: to });
    await refreshAfterPromptPresetEdit(active, `Preset renamed to "${to}"`);
    enginesPresetName.value = to;
  } catch (e) {
    toast(String(e), "error");
  }
});

enginesPresetDelete.addEventListener("click", async () => {
  const target = findPromptPreset(enginesPreset.value);
  if (!target || target.builtin) {
    toast("Pick one of your saved presets to delete", "error");
    return;
  }
  // Deleting the one in force returns the config to General and clears both
  // prompts; deleting another one leaves the selection alone.
  const inUse = target.name === activePromptPreset;
  const sure = await confirmDialog(
    inUse
      ? `Delete the preset "${target.name}"? It is in use, so both prompts return to the General default.`
      : `Delete the preset "${target.name}"?`
  );
  if (!sure) return;
  try {
    const active = await invoke("delete_prompt_preset", { name: target.name });
    await refreshAfterPromptPresetEdit(active, `Preset "${target.name}" deleted`);
    enginesPresetName.value = "";
  } catch (e) {
    toast(String(e), "error");
  }
});

function collectEngines(defaultEngine) {
  return {
    defaultEngine,
    ocrEngine: enginesOcr.value,
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
    // The prompt fields pass through un-trimmed: the Rust side trims and
    // treats a whitespace-only text as empty (the built-in default). The
    // preset selection rides with them: the Rust side keeps it only while
    // the texts stay that preset's wording.
    translationPrompt: enginesPromptTranslation.value,
    screenReadingPrompt: enginesPromptScreenReading.value,
    promptPreset: enginesPreset.value,
  };
}

// Ticket 39: the per-tab button's handler, extracted so the sticky footer's
// Save-all can call it — same validation, same toasts; the boolean just
// tells the footer whether the save went through.
async function saveEngines() {
  try {
    const active = await invoke("set_engines", { engines: collectEngines(enginesDefault.value) });
    applyPromptSelection(active);
    toast("Engines saved");
    return true;
  } catch (e) {
    // Ticket 51: the save was refused, so the config still holds the old
    // values — put the form back to them instead of leaving a rejected
    // switch (say, LM Studio with no model picked) looking applied.
    revertEnginesFields();
    toast(String(e), "error");
    return false;
  }
}

// "Reset to default" (ticket 37): clear the field AND save right away —
// empty means the built-in prompt applies again, so the reset is complete
// in one step (same save path and validation as Save engines). A rejected
// save (validation) reverts the whole engines form to the snapshot
// (ticket 51) — including the typed prompt this path cleared, so nothing
// on the tab shows values the config refused.
async function resetPromptToDefault(field, button) {
  const hadContent = field.value.trim() !== "";
  button.disabled = true;
  try {
    field.value = "";
    const active = await invoke("set_engines", { engines: collectEngines(enginesDefault.value) });
    // A cleared field under a preset leaves the preset: the config says
    // "custom" now, and the texts on show are the hand-written ones.
    applyPromptSelection(active);
    // Ticket 39: this saved the whole engines form as displayed, so the
    // engines baseline moves with it.
    snapshotFields(TRACKED_ENGINES);
    updateSaveFooter();
    toast(hadContent ? "Prompt reset — the built-in default applies again" : "Already using the built-in default");
  } catch (e) {
    revertEnginesFields();
    toast(String(e), "error");
  } finally {
    button.disabled = false;
  }
}
for (const [field, buttonId] of [
  [enginesPromptTranslation, "engines-prompt-translation-reset"],
  [enginesPromptScreenReading, "engines-prompt-screen-reading-reset"],
]) {
  document.getElementById(buttonId).addEventListener("click", (e) => {
    resetPromptToDefault(field, e.currentTarget);
  });
}

// The vision-reading latency/cost warning (ticket 12) follows the select
// live — the user sees what "fused" implies before saving it.
enginesReading.addEventListener("change", () => {
  enginesReadingWarning.classList.toggle("hidden", enginesReading.value !== "fused");
});

// Engine cards (ticket 54): the card of the engine picked in the Translation
// engine select is the expanded one; the rest stay reachable but collapse to
// their title bars. A title-bar click toggles any card, and switching the
// selection (or a rejected-save revert) moves the expansion to the newly
// selected card. Collapsed ≠ untracked: the footer's dirty tracker reads the
// fields themselves, so an edit made while a card was open still counts as
// unsaved changes after it collapses.
const engineCards = document.querySelectorAll(".engine-card[data-engine]");

function setCardExpanded(card, expanded) {
  card.classList.toggle("collapsed", !expanded);
  card.querySelector(".engine-title").setAttribute("aria-expanded", String(expanded));
}

function expandSelectedEngineCard() {
  for (const card of engineCards) {
    setCardExpanded(card, card.dataset.engine === enginesDefault.value);
  }
}

for (const card of engineCards) {
  const title = card.querySelector(".engine-title");
  title.setAttribute("role", "button");
  title.setAttribute("tabindex", "0");
  setCardExpanded(card, false);
  title.addEventListener("click", () => {
    setCardExpanded(card, card.classList.contains("collapsed"));
  });
  title.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      title.click();
    }
  });
}

enginesDefault.addEventListener("change", expandSelectedEngineCard);
// The select's pre-load markup value (Google) seeds the initial expansion;
// loadEngines re-syncs it to the saved value before the tab is used.
expandSelectedEngineCard();

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
        // The custom prompts pass through untouched — the wizard's upgrade
        // must never clear them.
        translationPrompt: savedEngines?.translationPrompt || "",
        screenReadingPrompt: savedEngines?.screenReadingPrompt || "",
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
// Translation Context (ticket 44): the LLM prompts carry this many of each
// zone's recent translations; 0 = off. Applies live — no loop restart.
const generalContext = document.getElementById("general-context");
const generalGenderless = document.getElementById("general-genderless");
const generalAutostart = document.getElementById("general-autostart");
// The window on-top toggles (ticket 34): saved with General, applied to
// the live windows immediately.
const generalResultsOnTop = document.getElementById("general-results-on-top");
const generalSettingsOnTop = document.getElementById("general-settings-on-top");
// Capture Visibility (ticket 46): saved with General, applied to the live
// windows immediately (the loop's masking reads it per capture — no
// restart).
const generalCaptureVisible = document.getElementById("general-capture-visible");
let generalLoaded = false;

function loadGeneral(status) {
  if (generalLoaded) return;
  generalLoaded = true;
  generalLanguage.value = status.language;
  generalCadence.value = status.cadenceMs;
  generalStability.value = status.stabilityMs;
  generalContext.value = status.contextTranslations;
  generalGenderless.checked = Boolean(status.genderless);
  generalAutostart.checked = Boolean(status.startWithWindows);
  generalResultsOnTop.checked = Boolean(status.resultsOnTop);
  generalSettingsOnTop.checked = Boolean(status.settingsOnTop);
  generalCaptureVisible.checked = Boolean(status.captureVisible);
}

// Ticket 39: the per-tab button's handler, extracted so the sticky footer's
// Save-all can call it — same validation, same toasts; the boolean just
// tells the footer whether the save went through.
async function saveGeneral() {
  const cadence = Math.round(Number(generalCadence.value) || 500);
  const stability = Math.round(Number(generalStability.value) || 0);
  // The context window clamps to the Rust side's cap (20) here, so a
  // fat-fingered value is corrected at the field instead of rejected.
  const context = Math.min(20, Math.max(0, Math.round(Number(generalContext.value) || 0)));
  try {
    await invoke("set_general", {
      settings: {
        language: generalLanguage.value,
        cadenceMs: cadence,
        stabilityMs: stability,
        contextTranslations: context,
        genderless: generalGenderless.checked,
        startWithWindows: generalAutostart.checked,
        resultsOnTop: generalResultsOnTop.checked,
        settingsOnTop: generalSettingsOnTop.checked,
        captureVisible: generalCaptureVisible.checked,
      },
    });
    toast("General settings saved");
    return true;
  } catch (e) {
    // Ticket 51: refused save — the form goes back to the stored values.
    restoreFields(TRACKED_GENERAL);
    updateSaveFooter();
    toast(String(e), "error");
    return false;
  }
}

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
const profilePreset = document.getElementById("profile-preset");
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
let profilePresetsLoaded = false; // the preset list the binding selector offers
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
  // Ticket 39: this redraw can move the app select programmatically (the
  // saved app may not be running, so the first running app is shown — what
  // is displayed is what Save would store). Re-baseline the select only if
  // the user had not moved it away from the snapshot; a user-chosen exe
  // stays dirty across the redraw.
  const before = profileExe.value;
  const chosen = profileExe.dataset.chosen || "";
  profileExe.replaceChildren();
  if (runningApps.length === 0) {
    const option = document.createElement("option");
    option.value = chosen;
    option.textContent = chosen || "no running apps — start the app, then ↻";
    profileExe.append(option);
  } else {
    for (const app of runningApps) {
      const option = document.createElement("option");
      option.value = app.exe;
      option.textContent = app.title ? `${app.title} — ${app.exe}` : app.exe;
      profileExe.append(option);
    }
    const match = runningApps.find((a) => a.exe.toLowerCase() === chosen.toLowerCase());
    profileExe.value = match ? match.exe : runningApps[0].exe;
  }
  if (savedSnapshot.get(profileExe) === before) {
    savedSnapshot.set(profileExe, profileExe.value);
  }
}

// The System Prompt Preset binding selector's options (ticket 43): the
// inherit choice plus every preset the Engines tab lists — built-ins
// first, then the saved ones. Rebuilt only when the option set changed,
// so a refresh never slams shut a dropdown the user has open (same rule
// as the Engines preset select).
function renderProfilePresetOptions() {
  const signature = promptPresets.map((preset) => `${preset.name}\u0001${preset.builtin}`).join("\u0000");
  if (profilePreset.dataset.sig !== signature) {
    const before = profilePreset.value;
    profilePreset.dataset.sig = signature;
    const inherit = document.createElement("option");
    inherit.value = "";
    inherit.textContent = "Inherit global (Settings → Engines)";
    const builtins = document.createElement("optgroup");
    builtins.label = "Built-in presets";
    const saved = document.createElement("optgroup");
    saved.label = "Your presets";
    for (const preset of promptPresets) {
      const option = document.createElement("option");
      option.value = preset.name;
      option.textContent = preset.label;
      (preset.builtin ? builtins : saved).append(option);
    }
    profilePreset.replaceChildren(inherit, builtins);
    if (saved.childElementCount > 0) profilePreset.append(saved);
    // Rebuilding resets the selection: put a still-existing choice back
    // (an unsaved user pick survives the redraw). A choice whose preset
    // vanished (deleted on the Engines tab mid-edit) falls back to
    // inherit — the backend reset the stored binding to match, and the
    // old name would only be rejected on save.
    profilePreset.value = [...profilePreset.options].some((o) => o.value === before)
      ? before
      : "";
  }
  // Follow the stored binding while the field is clean — a preset renamed
  // or deleted on the Engines tab moves (or resets) an untouched selector
  // to what the config now holds; a user-chosen value stays dirty across
  // the redraw (same rule as the exe select).
  const selected = profiles.find((p) => p.id === selectedProfileId);
  if (!fieldsDirty([profilePreset])) {
    profilePreset.value = selected?.promptPreset || "";
    snapshotFields([profilePreset]);
  }
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
  renderProfilePresetOptions();
  // Editor inputs only refill when the selection changes (or on explicit
  // save/create) — a status event must never clobber in-progress edits.
  if (selected && populatedProfileId !== selected.id) populateEditor(selected);
  if (!selected) {
    populatedProfileId = null;
    // Ticket 39: with no profile selected the editor is hidden and its
    // inputs are stale — re-baseline so leftover values never trip the
    // footer for a form nobody can save.
    snapshotFields(TRACKED_PROFILE);
  }
}

function populateEditor(profile) {
  populatedProfileId = profile.id;
  profileName.value = profile.name;
  profileExe.dataset.chosen = profile.exe;
  renderExeOptions();
  profileLanguage.value = profile.language;
  profileEngine.value = profile.engine || "";
  profileGenderless.value = profile.genderless || "inherit";
  renderProfilePresetOptions();
  profilePreset.value = profile.promptPreset || "";
  // Ticket 39: the freshly filled editor is the dirty baseline for this
  // profile — switching selection discards the previous edits (as it always
  // did), and the footer must follow.
  snapshotFields(TRACKED_PROFILE);
  updateSaveFooter();
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
        promptPreset: null,
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

// Ticket 39: the per-tab button's handler, extracted so the sticky footer's
// Save-all can call it — same validation, same toasts; the boolean just
// tells the footer whether the save went through.
async function saveProfile() {
  if (!selectedProfileId) return false;
  try {
    await invoke("update_profile", {
      id: selectedProfileId,
      profile: {
        name: profileName.value,
        exe: profileExe.value,
        language: profileLanguage.value,
        engine: profileEngine.value || null,
        genderless: profileGenderless.value,
        promptPreset: profilePreset.value || null,
      },
    });
    toast("Profile saved");
    renderProfiles(await invoke("get_status"));
    await renderProfileGlossary();
    return true;
  } catch (e) {
    // Ticket 51: refused save — revert the editor to the saved profile.
    // The exe select is the one restore that plain assignment can fail
    // silently: if the saved app has closed since the snapshot, its
    // option is gone and `value =` lands on no-selection. So re-point the
    // picker's bookkeeping (dataset.chosen) at the snapshot and rebuild
    // the options the way populateEditor does — a saved-but-not-running
    // app keeps its saved-ness instead of vanishing.
    restoreFields(TRACKED_PROFILE);
    profileExe.dataset.chosen = savedSnapshot.get(profileExe) ?? "";
    renderExeOptions();
    updateSaveFooter();
    toast(String(e), "error");
    return false;
  }
}

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

// Profile glossary export/import (ticket 33): the same dialogs, JSON
// format and import modes as the global Glossary — one file moves either
// direction. The buttons only exist inside the profile editor, so a
// profile is always selected when they are clickable.
document.getElementById("profile-glossary-export").addEventListener("click", async () => {
  if (!selectedProfileId) return;
  try {
    const path = await saveFileDialog({
      title: "Export profile glossary as JSON",
      defaultPath: "langception-glossary.json",
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!path) return;
    const count = await invoke("export_profile_glossary", { profileId: selectedProfileId, path });
    toast(`Exported ${count} term${count === 1 ? "" : "s"}`);
  } catch (e) {
    toast(String(e), "error");
  }
});

document.getElementById("profile-glossary-import").addEventListener("click", async () => {
  if (!selectedProfileId) return;
  try {
    const path = await openFileDialog({
      title: "Import profile glossary from JSON",
      multiple: false,
      directory: false,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!path) return;
    const mode = document.getElementById("profile-glossary-import-mode").value;
    const report = await invoke("import_profile_glossary", {
      profileId: selectedProfileId,
      path,
      mode,
    });
    toast(`Imported: ${report.added} added, ${report.updated} updated`);
    renderProfileGlossary();
  } catch (e) {
    toast(String(e), "error");
  }
});

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
const styleAlignmentRow = document.getElementById("style-alignment-row");
const styleTextAlignment = document.getElementById("style-text-alignment");
const styleTextVAlignment = document.getElementById("style-text-v-alignment");
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
  styleTextAlignment.value = style.textAlignment || "left";
  styleTextVAlignment.value = style.textVAlignment || "center";
  updateAlignmentRowVisibility();
  stylePlacement.value = style.placement || "over";
}

// Text Alignment (ticket 36) only means anything inside a Cover Source
// pill, so the selector is shown only while Cover Source is checked. The
// stored value stays put either way — unticking Cover Source never loses
// the chosen alignment.
function updateAlignmentRowVisibility() {
  styleAlignmentRow.classList.toggle("hidden", !styleCoverSource.checked);
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
    textAlignment: styleTextAlignment.value,
    textVAlignment: styleTextVAlignment.value,
    placement: stylePlacement.value,
  };
}

// Exact live preview (ticket 26): collectStyle() is the single source of
// truth — the exact object a save would store — pushed through the
// overlay's shared pill renderer (pill-style.js), so the preview cannot
// drift from what the In-place Overlay draws. Block geometry (Placement)
// is out of scope: it depends on each block's source rect, which settings
// does not have. With Cover Source on (tickets 36/38) the preview box
// stands in for the Text Block: the pill stretches across it — width for
// the horizontal axis (ticket 36), a fixed min-height for the vertical
// one (ticket 38; the overlay's stand-in is the block rect's height, which
// a preview does not have) — so the alignment inside shows exactly as the
// overlay draws it. The size is preview geometry, the inside layout is
// the shared technique. minHeight (not height) mirrors the overlay: text
// taller than the stand-in grows the pill, same as a wrapped block. Auto
// Colors sampling needs capture data, so the manual colors — the
// documented fallback — always show; the note under the pill says so
// while Auto Colors is on.
const stylePreviewPill = document.getElementById("style-preview-pill");
const stylePreviewText = document.getElementById("style-preview-text");
const stylePreviewNote = document.getElementById("style-preview-note");

function updateStylePreview() {
  const style = collectStyle();
  applyPillStyle(stylePreviewPill, style, null);
  if (style.background && style.coverSource) {
    stylePreviewPill.style.width = "100%";
    stylePreviewPill.style.maxWidth = "none";
    // Ticket 38: a stand-in block height, so top/center/bottom are all
    // visible — without it the pill hugs the text and only center shows.
    stylePreviewPill.style.minHeight = "96px";
  } else {
    stylePreviewPill.style.width = "";
    stylePreviewPill.style.maxWidth = "";
    stylePreviewPill.style.minHeight = "";
  }
  // The overlay's fitText always sets the size explicitly on the text
  // element; the preview does the same, at the configured size — which
  // since wrap-first fit (ticket 35) is also where the overlay wraps:
  // its shrink phase only answers to a block's height budget, geometry
  // this preview does not have. break-word mirrors fitText so a long
  // sample can't overflow the pill sideways.
  stylePreviewText.style.fontSize = style.fontSize + "px";
  stylePreviewText.style.overflowWrap = "break-word";
  stylePreviewNote.classList.toggle("hidden", !style.autoColors);
}

// Sliders fire a burst of input events; a short trailing debounce keeps the
// live feel without hammering config.toml per pixel.
function scheduleStyleSave() {
  clearTimeout(styleSaveTimer);
  styleSaveTimer = setTimeout(async () => {
    try {
      await invoke("set_text_style", { style: collectStyle() });
      // Ticket 39: Text Style persists immediately (no Save button), so the
      // tracker mirrors that — its baseline moves after every debounced save
      // and these fields practically never show the footer.
      snapshotFields(TRACKED_STYLE);
      updateSaveFooter();
    } catch (e) {
      // Ticket 51: the debounced save was refused — the controls go back to
      // the stored style, and the derived UI follows them (the color
      // field's disabled state, the alignment row, the preview that reads
      // these very fields).
      restoreFields(TRACKED_STYLE);
      styleColor.disabled = styleAutoColors.checked;
      updateAlignmentRowVisibility();
      updateStylePreview();
      updateSaveFooter();
      toast(String(e), "error");
    }
  }, 250);
}

for (const control of [styleFont, styleSize, styleAutoColors, styleColor, styleOutlineWidth, styleOutlineColor, styleBackground, styleOpacity, styleCoverSource, styleTextAlignment, styleTextVAlignment, stylePlacement]) {
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

// The alignment selector lives in the Cover Source context: it appears
// only while Cover Source is checked (the preview restyles via the
// shared listeners above, since the row's visibility does not change the
// collected style).
styleCoverSource.addEventListener("change", updateAlignmentRowVisibility);

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

// Ticket 39: the per-tab button's handler, extracted so the sticky footer's
// Save-all can call it — same validation, same toasts; the boolean just
// tells the footer whether the save went through.
async function saveHotkeys() {
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
    return true;
  } catch (e) {
    // Ticket 51: refused registration — the fields snap back to the
    // bindings the system actually holds.
    restoreFields(TRACKED_HOTKEYS);
    updateSaveFooter();
    toast(String(e), "error");
    return false;
  }
}

// Sticky Save footer + dirty tracking (ticket 39) -----------------------------
// One Save for every explicit-save area, pinned bottom-right and visible only
// while some tracked field on ANY tab differs from its last loaded/saved
// snapshot. Tracked: exactly the fields the four area saves read — Text Style
// is tracked too but re-baselines after every debounced auto-save, so it
// practically never trips the footer. Not tracked: the immediate-persistence
// flows (glossary rows, zone lists, profile create/delete),
// the wizard's own fields, and the About tab (nothing savable).

const saveFooter = document.getElementById("save-footer");
const TRACKED_GENERAL = [
  generalLanguage,
  generalCadence,
  generalStability,
  generalContext,
  generalGenderless,
  generalAutostart,
  generalResultsOnTop,
  generalSettingsOnTop,
  generalCaptureVisible,
];
const TRACKED_ENGINES = [
  enginesDefault,
  enginesOcr,
  enginesReading,
  enginesAzureKey,
  enginesAzureRegion,
  enginesDeeplKey,
  enginesOpenAiBaseUrl,
  enginesOpenAiKey,
  enginesOpenAiModel,
  enginesOpenAiTemperature,
  enginesLmStudioBaseUrl,
  enginesLmStudioModel,
  enginesLmStudioTemperature,
  enginesGeminiKey,
  enginesGeminiModel,
  enginesGeminiTemperature,
  enginesPromptTranslation,
  enginesPromptScreenReading,
  enginesPreset,
];
const TRACKED_PROFILE = [
  profileName,
  profileExe,
  profileLanguage,
  profileEngine,
  profileGenderless,
  profilePreset,
];
const TRACKED_HOTKEYS = [
  hotkeyInputs.toggle,
  hotkeyInputs.newZone,
  hotkeyInputs.pause,
  hotkeyInputs.adjust,
  hotkeyInputs.oneShot,
];
const TRACKED_STYLE = [
  styleFont,
  styleSize,
  styleAutoColors,
  styleColor,
  styleOutlineWidth,
  styleOutlineColor,
  styleBackground,
  styleOpacity,
  styleCoverSource,
  styleTextAlignment,
  styleTextVAlignment,
  stylePlacement,
];

// Ticket 39: element → field value at last load/save; selects snapshot by
// value (including the LM Studio "saved, not currently served" option).
const savedSnapshot = new Map();

function fieldValue(el) {
  return el.type === "checkbox" ? String(el.checked) : el.value;
}

function snapshotFields(fields) {
  for (const el of fields) savedSnapshot.set(el, fieldValue(el));
}

// Ticket 51: the restore half of the pair — after a rejected save, put the
// fields back to the snapshot so the form never keeps showing values the
// config refused (the toast explains why; the snap-back makes the
// rejection unmistakable). The snapshot stores each field's string, which
// is enough for plain inputs, selects and checkboxes; the LM Studio model
// dropdown is the one exception needing a rebuild — assigning a value
// with no matching option silently no-ops, so its options are rebuilt
// around the saved model first (renderLmStudioModels([], saved), the
// load-time precedent). Other dynamic-options selects stay restorable by
// plain assignment because their renderers never leave the value
// optionless. Callers add the derived-UI refreshes their fields drive —
// the footer at minimum, since programmatic writes fire no input/change
// events.
function restoreFields(fields) {
  for (const el of fields) {
    const saved = savedSnapshot.get(el);
    if (saved === undefined) continue;
    if (el === enginesLmStudioModel) {
      renderLmStudioModels([], saved);
    } else if (el.type === "checkbox") {
      el.checked = saved === "true";
    } else {
      el.value = saved;
    }
  }
}

// The engines revert's derived-UI steps, shared by the Settings form's
// failed set_engines paths (per-tab Save, footer Save-all, prompt reset —
// the wizard's upgrade deliberately does not revert: its payload is built
// from saved values and its fields are its own): the fused-reading
// warning follows the restored select, the card expansion follows the
// restored engine, the footer follows the restored fields.
function revertEnginesFields() {
  restoreFields(TRACKED_ENGINES);
  enginesReadingWarning.classList.toggle("hidden", enginesReading.value !== "fused");
  expandSelectedEngineCard();
  updateSaveFooter();
}

function fieldsDirty(fields) {
  return fields.some((el) => savedSnapshot.get(el) !== fieldValue(el));
}

function anyDirty() {
  return (
    fieldsDirty(TRACKED_GENERAL) ||
    fieldsDirty(TRACKED_ENGINES) ||
    fieldsDirty(TRACKED_PROFILE) ||
    fieldsDirty(TRACKED_HOTKEYS) ||
    fieldsDirty(TRACKED_STYLE)
  );
}

function updateSaveFooter() {
  saveFooter.classList.toggle("hidden", !anyDirty());
}

// Ticket 39: any edit anywhere flips the footer — the dirty state is
// tab-independent (edit Engines, wander to Hotkeys, the footer stays).
for (const el of [
  ...TRACKED_GENERAL,
  ...TRACKED_ENGINES,
  ...TRACKED_PROFILE,
  ...TRACKED_HOTKEYS,
  ...TRACKED_STYLE,
]) {
  el.addEventListener("input", updateSaveFooter);
  el.addEventListener("change", updateSaveFooter);
}

// One press commits every area whose fields drifted from their snapshot,
// reusing the per-area saves verbatim (validation included). Areas that
// already match are skipped, so a retry after a failed area does not re-save
// — and re-toast success for — the ones that went through; saved areas stay
// saved and a failed area reverts to its snapshot (ticket 51), so the
// footer only ever asks about genuinely unsaved edits.
async function saveAll() {
  for (const [fields, save] of [
    [TRACKED_ENGINES, saveEngines],
    [TRACKED_GENERAL, saveGeneral],
    [TRACKED_PROFILE, saveProfile],
    [TRACKED_HOTKEYS, saveHotkeys],
  ]) {
    if (!fieldsDirty(fields)) continue;
    if (await save()) snapshotFields(fields);
  }
  updateSaveFooter();
}
document.getElementById("save-all").addEventListener("click", saveAll);

// Ticket 39: the close handshake's page side. CloseRequested (titlebar ✕,
// Alt+F4) no longer hides this window directly — it pings the page, and the
// page decides: a clean page hides immediately, a dirty page asks first and
// hides only on confirm. The window only hides either way, so edits survive
// a cancelled dismiss and a reopen keeps them. Registered top-level (not in
// the boot handler) so the ping is never missed while get_status is still in
// flight; the flag keeps a second ✕ from stacking dialogs.
let closeAskOpen = false;
async function handleSettingsCloseRequest() {
  if (closeAskOpen) return;
  if (!anyDirty()) {
    invoke("hide_settings").catch((e) => toast(String(e), "error"));
    return;
  }
  closeAskOpen = true;
  try {
    const discard = await askDialog("Discard unsaved changes?", {
      title: "Langception — Settings",
      kind: "warning",
    });
    if (discard) invoke("hide_settings").catch((e) => toast(String(e), "error"));
  } catch (e) {
    // A failed dialog must not strand the window silently — surface it and
    // let the user try again (the window simply stays open, edits intact).
    toast(String(e), "error");
  } finally {
    closeAskOpen = false;
  }
}
// Registered through WebviewWindow, not the plain event API imported above:
// plain listen is EventTarget::Any, which receives every emit regardless of
// emit_to targeting (ADR-0003's event-targeting lesson) — the handshake ping
// must stay addressed to THIS window, and the label-filtered listener
// guarantees it.
window.__TAURI__.webviewWindow
  .getCurrentWebviewWindow()
  .listen("settings-request-close", handleSettingsCloseRequest);

// Update Check (ticket 28, CONTEXT.md) ---------------------------------------
// The About tab's check-only version check: a manual button, an opt-in
// launch-time check, and — for a newer version — a link to its download
// page. Every outcome renders gently in the result area; the launch-time
// check arrives as an event (while the page is open) or rides the boot
// status (it finished earlier), so it surfaces exactly as a manual check.

const aboutCheckUpdates = document.getElementById("about-check-updates");
const aboutUpdateOnLaunch = document.getElementById("about-update-on-launch");
const aboutUpdateResult = document.getElementById("about-update-result");
const aboutUpdateDownloadRow = document.getElementById("about-update-download-row");
const aboutUpdateDownload = document.getElementById("about-update-download");

// The download URL of the newest reported result; only a "new-version"
// outcome carrying one shows the action, and only its URL ever opens.
let updateDownloadUrl = null;

function renderUpdateCheck(outcome) {
  aboutUpdateResult.classList.remove("hidden");
  aboutUpdateDownloadRow.classList.add("hidden");
  updateDownloadUrl = null;
  switch (outcome.kind) {
    case "up-to-date":
      aboutUpdateResult.textContent = "You are on the latest version.";
      break;
    case "new-version": {
      const parts = [`New version available: ${outcome.version}`];
      if (outcome.released) parts.push(`Released: ${outcome.released}`);
      aboutUpdateResult.textContent = parts.join(" · ");
      if (outcome.downloadUrl) {
        updateDownloadUrl = outcome.downloadUrl;
        aboutUpdateDownloadRow.classList.remove("hidden");
      }
      break;
    }
    case "not-listed":
      aboutUpdateResult.textContent =
        "The update server answered, but no update information for Langception is published there yet.";
      break;
    case "unavailable":
      aboutUpdateResult.textContent =
        "Update check unavailable — no update URL is configured (the feature is disabled).";
      break;
    case "error":
      aboutUpdateResult.textContent = `Update check unavailable — ${outcome.message}`;
      break;
    default:
      aboutUpdateResult.textContent = "";
  }
}

aboutCheckUpdates.addEventListener("click", async () => {
  aboutCheckUpdates.disabled = true;
  aboutUpdateResult.classList.remove("hidden");
  aboutUpdateDownloadRow.classList.add("hidden");
  aboutUpdateResult.textContent = "Checking…";
  try {
    renderUpdateCheck(await invoke("check_for_updates"));
  } catch (e) {
    aboutUpdateResult.textContent = "";
    toast(String(e), "error");
  } finally {
    aboutCheckUpdates.disabled = false;
  }
});

// The toggle persists immediately (like Start-with-Windows) so the
// checkbox never lies about what the next launch will do; a failed save
// reverts it.
aboutUpdateOnLaunch.addEventListener("change", async () => {
  const wanted = aboutUpdateOnLaunch.checked;
  aboutUpdateOnLaunch.disabled = true;
  try {
    await invoke("set_update_check_on_launch", { enabled: wanted });
    toast(wanted
      ? "Langception will check for a new version at launch"
      : "Launch-time update checks disabled");
  } catch (e) {
    aboutUpdateOnLaunch.checked = !wanted; // revert
    toast(String(e), "error");
  } finally {
    aboutUpdateOnLaunch.disabled = false;
  }
});

aboutUpdateDownload.addEventListener("click", () => {
  if (!updateDownloadUrl) return;
  invoke("open_update_page", { url: updateDownloadUrl }).catch((e) =>
    toast(String(e), "error")
  );
});

// The launch-time check's live surface: same rendering as a manual check,
// never a toast — failures stay silent.
listen("update-check-result", (event) => renderUpdateCheck(event.payload)).catch((e) =>
  toast(String(e), "error")
);

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
  // Update Check (ticket 28): the saved launch toggle plus whatever check
  // already ran (a launch-time one that finished before this page opened).
  aboutUpdateOnLaunch.checked = Boolean(status.updateCheckOnLaunch);
  if (status.lastUpdateCheck) renderUpdateCheck(status.lastUpdateCheck);

  // Ticket 39: the page just loaded, so everything shown is the saved state
  // — baseline the tracker from it. Engines re-baselines when its lazy load
  // fills the fields, Profiles inside populateEditor/renderProfiles.
  snapshotFields(TRACKED_GENERAL);
  snapshotFields(TRACKED_STYLE);
  snapshotFields(TRACKED_HOTKEYS);
  snapshotFields(TRACKED_ENGINES);
  updateSaveFooter();

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
