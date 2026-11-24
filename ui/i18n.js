// Langception i18n seam (ticket 62): the string-extraction layer every
// future UI language plugs into. `t(key, params)` resolves a key against
// the active dictionary, falling back to the English dictionary and then
// to the key itself; `applyStatic(root)` applies `data-i18n` (element
// text), `data-i18n-html` (rich-text hints whose English carries inline
// <strong>/<code> markup — dictionaries are app-bundled, the same trust
// level as the pages), `data-i18n-placeholder` and `data-i18n-title`.
// Dictionaries are plain JSON beside this file (`i18n/en.json`,
// `i18n/<lang>.json`) fetched relative to the page — data, not code, so a
// Rust test can parse both files to pin key parity (the repo has no JS
// test harness). No build step: classic script, loaded before each page's
// own script.
//
// Bootstrap: each window resolves its language through an explicit seam —
// `uiLanguage` on the status it already fetches (Settings, Results) or one
// `get_ui_language` invoke (Debug Log, picker, overlays) — and calls
// `I18n.init(lang)`. `I18n.ready` resolves once that happened, so
// titlebar.js (loaded before the page script) can wait on it.
(function () {
  "use strict";

  let lang = "en";
  let en = null; // the English dictionary — the fallback
  let dict = {}; // the active language's dictionary

  function lookup(d, key) {
    return d && Object.prototype.hasOwnProperty.call(d, key) ? d[key] : undefined;
  }

  // {name} placeholders — string interpolation only (Thai does not
  // inflect plurals; no ICU machinery in v1).
  function interpolate(template, params) {
    if (!params) return template;
    return template.replace(/\{(\w+)\}/g, (match, name) =>
      Object.prototype.hasOwnProperty.call(params, name) ? String(params[name]) : match
    );
  }

  function t(key, params) {
    let value = lookup(dict, key);
    if (value === undefined) value = lookup(en, key);
    // Missing everywhere: the key itself — a visible placeholder that can
    // never blank out the UI.
    if (value === undefined) return key;
    return interpolate(value, params);
  }

  // Static-markup applier. A key missing from BOTH dictionaries is
  // skipped so a typo can never blank an element (the markup's own text
  // stays).
  function applyStatic(root) {
    const scope = root || document;
    const known = (key) => lookup(dict, key) !== undefined || lookup(en, key) !== undefined;
    for (const el of scope.querySelectorAll("[data-i18n]")) {
      if (!known(el.dataset.i18n)) continue;
      el.textContent = t(el.dataset.i18n);
    }
    for (const el of scope.querySelectorAll("[data-i18n-html]")) {
      if (!known(el.dataset.i18nHtml)) continue;
      el.innerHTML = t(el.dataset.i18nHtml);
    }
    for (const el of scope.querySelectorAll("[data-i18n-placeholder]")) {
      if (!known(el.dataset.i18nPlaceholder)) continue;
      el.placeholder = t(el.dataset.i18nPlaceholder);
    }
    for (const el of scope.querySelectorAll("[data-i18n-title]")) {
      if (!known(el.dataset.i18nTitle)) continue;
      el.title = t(el.dataset.i18nTitle);
    }
  }

  async function loadJson(name) {
    const response = await fetch(`i18n/${name}`);
    if (!response.ok) throw new Error(`cannot load i18n/${name}: ${response.status}`);
    return response.json();
  }

  // Resolves the dictionaries for `language` (a UiLanguage code: "en" |
  // "th"), applies the static markup, and resolves I18n.ready. A missing
  // or broken non-English dictionary degrades to English (the graceful
  // fallback ticket 62 ships with — th.json is the empty placeholder
  // until ticket 63 fills it). Safe to call again (a live language
  // switch re-applies).
  async function init(language) {
    lang = language === "th" ? "th" : "en";
    if (!en) en = await loadJson("en.json");
    dict = lang === "en" ? en : ((await loadJson(`${lang}.json`).catch(() => ({}))) || {});
    applyStatic(document);
    document.documentElement.lang = lang;
    return lang;
  }

  let readyResolve = null;
  const readyPromise = new Promise((resolve) => {
    readyResolve = resolve;
  });

  window.I18n = {
    t,
    applyStatic,
    // Wraps init: the one call each page's script makes once its language
    // is resolved. readyResolve is set in the constructor above, so it is
    // never null here.
    init: async (language) => {
      try {
        return await init(language);
      } finally {
        readyResolve();
      }
    },
    ready: readyPromise,
    language: () => lang,
  };
})();
