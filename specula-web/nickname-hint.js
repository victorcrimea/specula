// The (i) beside a nickname input and the tip it shows with the naming
// rule. The browser's own bubble only says the name is wrong, not why, so
// a refused name opens the tip instead and keeps it open until the name
// passes.
//
//   <span class="nickname-hint">
//     <label>... <input></label>
//     <button class="hint-icon" type="button">i</button>
//     <span class="hint-tip" role="tooltip" hidden>rule</span>
//   </span>
//
// `speculaNicknameHint(input)` opens it from a script, for a name the
// server refused.
(() => {
  const hints = new Map();

  const setUp = (wrapper) => {
    const input = wrapper.querySelector("input");
    const icon = wrapper.querySelector(".hint-icon");
    const tip = wrapper.querySelector(".hint-tip");
    if (!input || !icon || !tip) return;
    let pinned = false;
    let hovered = false;

    // Fixed, as the setup page's spectators box scrolls and would clip it.
    const place = () => {
      const anchor = input.getBoundingClientRect();
      tip.style.top = anchor.bottom + 6 + "px";
      const room = document.documentElement.clientWidth - tip.offsetWidth - 8;
      tip.style.left = Math.max(8, Math.min(anchor.left, room)) + "px";
    };
    const update = () => {
      const shown = pinned || hovered;
      tip.hidden = !shown;
      icon.setAttribute("aria-expanded", String(shown));
      if (shown) place();
    };
    const pin = () => {
      pinned = true;
      update();
    };

    icon.addEventListener("mouseenter", () => { hovered = true; update(); });
    icon.addEventListener("mouseleave", () => { hovered = false; update(); });
    icon.addEventListener("focus", () => { hovered = true; update(); });
    icon.addEventListener("blur", () => { hovered = false; update(); });
    // Touch has no hover, so a tap toggles it.
    icon.addEventListener("click", () => {
      pinned = !pinned;
      update();
    });
    input.addEventListener("invalid", (event) => {
      event.preventDefault();
      input.focus();
      pin();
    });
    input.addEventListener("input", () => {
      if (pinned && input.checkValidity()) {
        pinned = false;
        update();
      }
    });
    document.addEventListener("pointerdown", (event) => {
      if (pinned && !wrapper.contains(event.target)) {
        pinned = false;
        update();
      }
    });
    const follow = () => { if (!tip.hidden) place(); };
    window.addEventListener("resize", follow);
    document.addEventListener("scroll", follow, true);
    hints.set(input, pin);
  };

  window.speculaNicknameHint = (input) => hints.get(input)?.();

  const start = () => document.querySelectorAll(".nickname-hint").forEach(setUp);
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
  else start();
})();
