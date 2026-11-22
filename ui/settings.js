// Langception Settings window — vanilla JS against the Tauri global API.
// One surface for every config: General, Engines (OCR pack detection),
// Glossary (full management + JSON import/export), Text Style, Hotkeys
// (rebinds apply live), About. Doubles as the first-run wizard host: when
// status.firstRun the wizard view shows before the tabs and is skippable.
// No build step.

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

// General -------------------------------------------------------------------

const generalLanguage = document.getElementById("general-language");
const generalCadence = document.getElementById("general-cadence");
const generalStability = document.getElementById("general-stability");
const generalAutostart = document.getElementById("general-autostart");
let generalLoaded = false;

function loadGeneral(status) {
  if (generalLoaded) return;
  generalLoaded = true;
  generalLanguage.value = status.language;
  generalCadence.value = status.cadenceMs;
  generalStability.value = status.stabilityMs;
  generalAutostart.checked = Boolean(status.startWithWindows);
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

// Text Style -----------------------------------------------------------------

const styleFont = document.getElementById("style-font");
const styleSize = document.getElementById("style-size");
const styleColor = document.getElementById("style-color");
const styleOutlineWidth = document.getElementById("style-outline-width");
const styleOutlineColor = document.getElementById("style-outline-color");
const styleBackground = document.getElementById("style-background");
const styleOpacity = document.getElementById("style-opacity");
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
  styleColor.value = normalizeHex(style.color, "#ffffff");
  styleOutlineWidth.value = style.outlineWidth;
  styleOutlineColor.value = normalizeHex(style.outlineColor, "#000000");
  styleBackground.checked = style.background;
  styleOpacity.value = style.backgroundOpacity;
}

function collectStyle() {
  return {
    fontFamily: styleFont.value.trim() || "Noto Sans Thai",
    fontSize: Number(styleSize.value) || 16,
    color: normalizeHex(styleColor.value, "#ffffff"),
    outlineWidth: Number(styleOutlineWidth.value) || 0,
    outlineColor: normalizeHex(styleOutlineColor.value, "#000000"),
    background: styleBackground.checked,
    backgroundOpacity: Number(styleOpacity.value) || 0,
  };
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

for (const control of [styleFont, styleSize, styleColor, styleOutlineWidth, styleOutlineColor, styleBackground, styleOpacity]) {
  control.addEventListener("input", scheduleStyleSave);
  control.addEventListener("change", scheduleStyleSave);
}

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
  } catch (e) {
    toast(String(e), "error");
  }
}

document.getElementById("about-open-config").addEventListener("click", () => {
  invoke("open_config_folder").catch((e) => toast(String(e), "error"));
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
  loadHotkeys(status.hotkeys);
  renderGlossary();
  loadAbout();
  updateChips(status);

  if (status.firstRun) {
    showWizard();
  } else {
    finishWizard();
  }

  await listen("engine-state", (event) => {
    updateChips(event.payload);
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
