// The feedback form: a button (among the custom buttons next to Warm up on
// the game page, a corner button elsewhere) opens a window with a text box, an optional contact and
// a screenshot of the page as it was when the window opened. A seated
// player's game is paused while it is open, through the game menu's own
// Pause, so the pause costs what any pause costs. Sent to `/feedback`.
(() => {
  const T = (text) => (window.speculaT ? window.speculaT(text) : text);
  const MAX_SHOT_WIDTH = 1280;
  const SHOT_QUALITY = 0.7;
  const MAX_TEXT = 2000;
  const MAX_CONTACT = 200;

  const style = document.createElement("style");
  style.textContent = `
#feedback-corner { position: fixed; right: 12px; bottom: 12px; z-index: 40; font: 12px system-ui, sans-serif;
  color: #e6e9eb; background: rgba(15, 18, 20, 0.85); border: 1px solid #3a4248; border-radius: 4px;
  padding: 3px 8px; cursor: pointer; opacity: 0.75; }
#feedback-corner:hover { opacity: 1; border-color: #8fa3b0; }
#feedback-window { max-width: min(480px, calc(100vw - 32px)); width: 100%; box-sizing: border-box; padding: 16px;
  color: #e6e9eb; background: #1b2024; border: 1px solid #3a4248; border-radius: 6px;
  font: 14px system-ui, sans-serif; cursor: default; }
#feedback-window::backdrop { background: rgba(0, 0, 0, 0.55); }
#feedback-window h2 { margin: 0 0 10px; font-size: 16px; font-weight: 600; }
#feedback-window label { display: block; margin: 10px 0 4px; }
#feedback-window textarea, #feedback-window input[type=text] { box-sizing: border-box; width: 100%; font: inherit;
  color: #e6e9eb; background: #0f1214; border: 1px solid #3a4248; border-radius: 4px; padding: 6px 8px; }
#feedback-window textarea { height: 7em; resize: vertical; }
#feedback-window .attach { display: flex; align-items: center; gap: 6px; }
#feedback-window .attach[hidden], #feedback-window .note[hidden], #feedback-window img[hidden] { display: none; }
#feedback-window img { display: block; max-width: 100%; max-height: 120px; margin-top: 6px; border: 1px solid #3a4248; }
#feedback-window .note, #feedback-window .status { margin: 8px 0 0; color: #a9b4bb; font-size: 12px; }
#feedback-window .status.error { color: #e8a0a0; }
#feedback-window .buttons { display: flex; justify-content: flex-end; gap: 8px; margin-top: 14px; }
#feedback-window button { font: inherit; color: #e6e9eb; background: #0f1214; border: 1px solid #3a4248;
  border-radius: 4px; padding: 4px 14px; cursor: pointer; }
#feedback-window button:disabled { opacity: 0.5; cursor: default; }
`;
  document.head.append(style);

  const element = (tag, props = {}, ...children) => {
    const node = Object.assign(document.createElement(tag), props);
    node.append(...children);
    return node;
  };

  // The page's canvases, copied in the frame's own task: a GPU canvas holds
  // its picture only until the frame is composited, and neither html2canvas
  // nor a later drawImage would see it.
  const grabCanvases = () =>
    new Promise((resolve) => {
      requestAnimationFrame(() => {
        const copies = [];
        for (const canvas of document.querySelectorAll("canvas")) {
          const rect = canvas.getBoundingClientRect();
          if (!canvas.width || !canvas.height || !rect.width || !rect.height) {
            copies.push(null);
            continue;
          }
          try {
            const copy = element("canvas", { width: canvas.width, height: canvas.height });
            copy.getContext("2d").drawImage(canvas, 0, 0);
            copies.push({ copy, rect });
          } catch {
            copies.push(null);
          }
        }
        resolve(copies);
      });
    });

  const toJpeg = (canvas) =>
    new Promise((resolve) => canvas.toBlob((blob) => resolve(blob), "image/jpeg", SHOT_QUALITY));

  // The whole page by html2canvas, with the canvases it cannot read put in
  // from the copies; without it, the copies alone on the page background.
  const screenshot = async (copies, cloned) => {
    const scale = Math.min(1, MAX_SHOT_WIDTH / window.innerWidth);
    let canvas = null;
    if (window.html2canvas) {
      try {
        canvas = await window.html2canvas(document.body, {
          backgroundColor: "#0f1214",
          logging: false,
          scale,
          width: window.innerWidth,
          height: window.innerHeight,
          windowWidth: window.innerWidth,
          windowHeight: window.innerHeight,
          x: window.scrollX,
          y: window.scrollY,
          ignoreElements: (node) => node.id === "feedback-window",
          onclone: (doc) => {
            [...doc.querySelectorAll("canvas")].forEach((target, index) => {
              const grabbed = copies[index];
              if (!grabbed) return;
              target.width = grabbed.copy.width;
              target.height = grabbed.copy.height;
              target.getContext("2d").drawImage(grabbed.copy, 0, 0);
            });
            cloned();
          },
        });
      } catch (error) {
        console.error("feedback screenshot:", error);
        canvas = null;
      }
    }
    if (!canvas) {
      canvas = element("canvas", {
        width: Math.round(window.innerWidth * scale),
        height: Math.round(window.innerHeight * scale),
      });
      const context = canvas.getContext("2d");
      context.fillStyle = "#0f1214";
      context.fillRect(0, 0, canvas.width, canvas.height);
      for (const grabbed of copies) {
        if (!grabbed) continue;
        const { rect, copy } = grabbed;
        context.drawImage(copy, rect.left * scale, rect.top * scale, rect.width * scale, rect.height * scale);
      }
    }
    return toJpeg(canvas);
  };

  const gameMenuPause = () => {
    const menu = document.getElementById("game-menu");
    const button = document.getElementById("game-menu-pause");
    return menu && !menu.hidden && button ? button : null;
  };

  // The button says "Resume" once the server has the seat's pause.
  const pausedByUs = () => {
    const button = gameMenuPause();
    if (!button || button.disabled || button.dataset.i18nEn !== "Pause") return false;
    button.click();
    return true;
  };

  const resumeOurs = () => {
    let tries = 0;
    const attempt = () => {
      const button = gameMenuPause();
      if (button && !button.disabled && button.dataset.i18nEn === "Resume") {
        button.click();
      } else if (++tries < 30) {
        setTimeout(attempt, 100);
      }
    };
    attempt();
  };

  let current = null;

  const open = () => {
    if (current) return;
    const state = { paused: false, shot: null, closed: false };
    const dialog = element("dialog", { id: "feedback-window" });
    const title = element("h2");
    const textLabel = element("label", { htmlFor: "feedback-text" });
    const text = element("textarea", { id: "feedback-text", maxLength: MAX_TEXT });
    const contactLabel = element("label", { htmlFor: "feedback-contact" });
    const contact = element("input", { id: "feedback-contact", type: "text", maxLength: MAX_CONTACT, autocomplete: "off" });
    const attachBox = element("input", { type: "checkbox", checked: true });
    const attachText = element("span");
    const attach = element("label", { className: "attach", hidden: true }, attachBox, attachText);
    const preview = element("img", { hidden: true, alt: "" });
    const note = element("p", { className: "note", hidden: true });
    const status = element("p", { className: "status", hidden: true });
    const cancel = element("button", { type: "button" });
    const send = element("button", { type: "submit" });
    const form = element("form", { method: "dialog" }, title, textLabel, text, contactLabel, contact, attach, preview, note, status, element("div", { className: "buttons" }, cancel, send));
    dialog.append(form);
    const translate = () => {
      title.textContent = T("Send feedback");
      textLabel.textContent = T("What happened, or what would you change?");
      contactLabel.textContent = T("Contact (optional)");
      attachText.textContent = T("Attach a screenshot of the page");
      note.textContent = T("The game is paused while this window is open, which uses your pause time.");
      cancel.textContent = T("Cancel");
      send.textContent = T("Send");
    };
    translate();
    document.body.append(dialog);

    const close = () => {
      if (state.closed) return;
      state.closed = true;
      current = null;
      if (state.paused) resumeOurs();
      dialog.close();
      dialog.remove();
    };

    // Before the window or the pause changes the picture.
    let resolveCloned;
    const cloned = new Promise((resolve) => (resolveCloned = resolve));
    grabCanvases().then((copies) => {
      state.shot = screenshot(copies, resolveCloned).finally(resolveCloned);
      state.shot.then((blob) => {
        if (state.closed || !blob) return;
        state.blob = blob;
        state.url = URL.createObjectURL(blob);
        preview.src = state.url;
        preview.hidden = false;
        attach.hidden = false;
      });
    });
    cloned.then(() => {
      if (state.closed) return;
      if (pausedByUs()) {
        state.paused = true;
        note.hidden = false;
      }
    });

    // The game's own keys must not hear the typing.
    dialog.addEventListener("keydown", (event) => event.stopPropagation());
    dialog.addEventListener("keyup", (event) => event.stopPropagation());
    dialog.addEventListener("cancel", (event) => {
      event.preventDefault();
      close();
    });
    cancel.addEventListener("click", close);
    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      if (!text.value.trim()) {
        text.focus();
        return;
      }
      send.disabled = true;
      status.className = "status";
      status.hidden = true;
      try {
        let blob = null;
        if (attachBox.checked && !attach.hidden) {
          await state.shot;
          blob = state.blob || null;
        }
        const id = location.pathname.match(/[0-9a-f]{8}-[0-9a-f-]{27}/);
        const header = new TextEncoder().encode(
          JSON.stringify({
            text: text.value,
            contact: contact.value,
            page: location.pathname.replace(/[0-9a-f]{8}-[0-9a-f-]{27}/, ":id"),
            match: id ? id[0] : "-",
          }),
        );
        const length = new Uint8Array(4);
        new DataView(length.buffer).setUint32(0, header.length, true);
        const response = await fetch("/feedback", {
          method: "POST",
          body: new Blob(blob ? [length, header, blob] : [length, header]),
        });
        if (!response.ok) throw new Error(await response.text());
        form.replaceChildren(
          title,
          element("p", {}, T("Thank you for your feedback!")),
          element("div", { className: "buttons" }, cancel),
        );
        cancel.textContent = T("Close");
      } catch (error) {
        console.error("feedback:", error);
        status.textContent = T("Could not send the feedback. Try again later.");
        status.className = "status error";
        status.hidden = false;
        send.disabled = false;
      }
    });
    dialog.addEventListener("close", () => {
      if (state.url) URL.revokeObjectURL(state.url);
    });
    window.speculaOnLanguage?.(translate);
    current = { close };
    dialog.showModal();
    text.focus();
  };

  const place = () => {
    const label = (button) => {
      const set = () => (button.textContent = T("Feedback"));
      window.speculaOnLanguage ? window.speculaOnLanguage(set) : set();
    };
    // The game page's custom buttons, next to Warm up; its own styling.
    const controls = document.getElementById("controls");
    if (controls) {
      const button = element("button", { id: "feedback-open", type: "button" });
      label(button);
      button.addEventListener("click", open);
      const warmup = document.getElementById("warmup");
      if (warmup) warmup.after(button);
      else controls.append(button);
      return;
    }
    const corner = element("button", { id: "feedback-corner", type: "button" });
    label(corner);
    corner.addEventListener("click", open);
    document.body.append(corner);
  };

  // The screenshot's library, fetched once the page is idle so that the
  // window opens at the click.
  const loadLibrary = () => {
    if (window.html2canvas) return;
    document.head.append(element("script", { src: "/html2canvas.min.js", async: true }));
  };

  const start = () => {
    place();
    (window.requestIdleCallback || ((callback) => setTimeout(callback, 3000)))(loadLibrary);
  };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", start);
  else start();
})();
