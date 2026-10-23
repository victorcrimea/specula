// Translates the page's own text. The English stays in the markup, so the
// page reads fine until the wasm has loaded a language; the wasm then sets
// `speculaTranslate` and calls `speculaApplyLanguage` after each load.
//
//   data-i18n            the element's first text node
//   data-i18n-html       its inner HTML, where a translation holds markup
//   data-i18n-attrs      the attributes it names, space separated
//
// The English each one was written with is kept in `data-i18n-en`
// (`data-i18n-en-<attribute>`), where the wasm also puts the text it sets
// itself (`lang::set_text`), so a later language starts from the English.
(() => {
  const squash = (text) => text.replace(/\s+/g, " ").trim();
  const translate = (text) => (window.speculaTranslate ? window.speculaTranslate(text) : text);
  const listeners = [];

  // For scripts that build text: `speculaT("Fold the details")`, and
  // `speculaOnLanguage(fn)` to make it again when the language changes.
  window.speculaT = translate;
  window.speculaOnLanguage = (listener) => {
    listeners.push(listener);
    listener();
  };

  const textNode = (element) =>
    [...element.childNodes].find((node) => node.nodeType === Node.TEXT_NODE && node.nodeValue.trim());

  window.speculaApplyLanguage = () => {
    for (const element of document.querySelectorAll("[data-i18n]")) {
      const node = textNode(element);
      if (!node) continue;
      if (element.dataset.i18nEn === undefined) element.dataset.i18nEn = squash(node.nodeValue);
      const lead = node.nodeValue.match(/^\s*/)[0];
      const trail = node.nodeValue.match(/\s*$/)[0];
      node.nodeValue = lead + translate(element.dataset.i18nEn) + trail;
    }
    for (const element of document.querySelectorAll("[data-i18n-html]")) {
      if (element.dataset.i18nEn === undefined) element.dataset.i18nEn = squash(element.innerHTML);
      // The size the server fills in, which a translation names {size}.
      element.innerHTML = translate(element.dataset.i18nEn).replace("{size}", element.dataset.size ?? "");
    }
    for (const element of document.querySelectorAll("[data-i18n-attrs]")) {
      for (const name of element.dataset.i18nAttrs.split(" ")) {
        const key = "i18nEn" + name.replace(/(^|-)(\w)/g, (_, __, c) => c.toUpperCase());
        if (element.dataset[key] === undefined) element.dataset[key] = element.getAttribute(name) ?? "";
        element.setAttribute(name, translate(element.dataset[key]));
      }
    }
    for (const listener of listeners) listener();
  };
})();
