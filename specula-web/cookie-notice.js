// The cookie notice, once per browser: one cookie holds a player's seat, and
// the visitor is asked whether usage may be logged with it (`/consent`).
// Until they allow it, the server logs no visitor id.
(() => {
  const KEY = "specula-consent-asked";
  const T = (text) => (window.speculaT ? window.speculaT(text) : text);
  try {
    if (localStorage.getItem(KEY)) return;
  } catch {
    // Storage blocked: the notice comes back each visit, which is the safe side.
  }
  const style = document.createElement("style");
  style.textContent = `
#cookie-notice { position: fixed; left: 12px; bottom: 12px; z-index: 45; max-width: min(360px, calc(100vw - 24px));
  box-sizing: border-box; padding: 8px 10px; display: flex; flex-wrap: wrap; align-items: center; gap: 8px 10px;
  font: 12px system-ui, sans-serif; color: #e6e9eb; background: rgba(15, 18, 20, 0.92);
  border: 1px solid #3a4248; border-radius: 4px; }
#cookie-notice span { flex: 1 1 100%; }
#cookie-notice button { font: inherit; color: #e6e9eb; background: #0f1214;
  border: 1px solid #3a4248; border-radius: 4px; padding: 3px 10px; cursor: pointer; }
#cookie-notice button:hover { border-color: #8fa3b0; }
`;
  const text = document.createElement("span");
  const allow = document.createElement("button");
  const decline = document.createElement("button");
  allow.type = decline.type = "button";
  const notice = document.createElement("div");
  notice.id = "cookie-notice";
  notice.setAttribute("role", "note");
  notice.append(text, allow, decline);
  const translate = () => {
    text.textContent = T(
      "A cookie keeps your seat in a game. May we also count visits and games, anonymously, to improve the site?",
    );
    allow.textContent = T("Allow");
    decline.textContent = T("Decline");
  };
  const answer = (yes) => {
    fetch("/consent", { method: "POST", body: yes ? "1" : "0" }).catch(() => {});
    try {
      localStorage.setItem(KEY, "1");
    } catch {
      // Not remembered; it asks again next visit.
    }
    notice.remove();
  };
  allow.addEventListener("click", () => answer(true));
  decline.addEventListener("click", () => answer(false));
  const start = () => {
    document.head.append(style);
    document.body.append(notice);
    window.speculaOnLanguage ? window.speculaOnLanguage(translate) : translate();
  };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
  else start();
})();
