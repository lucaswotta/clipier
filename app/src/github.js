import { invoke } from "@tauri-apps/api/core";
import { escapeHtml } from "./text.js";
import { words } from "./vocabulary.js";

/** What each pull request panel on screen knows, by the key of the branch it is for. It outlives a repaint. */
const states = new Map();
const infos = new Map();

const stateOf = (key) => {
  if (!states.has(key)) states.set(key, { step: "idle", error: "" });
  return states.get(key);
};

/** The place on a screen where the panel of `key` goes. */
export const pullRequestSlot = (key) => `<div class="pr" data-pr="${escapeHtml(key)}"></div>`;

/** Draws the panels in `root`; `list` maps each key to `{ repository, branch, base, title, body }`. */
export function mountPullRequests(root, list) {
  for (const [key, info] of Object.entries(list)) infos.set(key, info);
  for (const slot of root.querySelectorAll("[data-pr]")) draw(slot.dataset.pr);
}

function slotOf(key) {
  return [...document.querySelectorAll("[data-pr]")].find((el) => el.dataset.pr === key);
}

function redraw(key) {
  if (slotOf(key)) draw(key);
}

const copy = (button, text) =>
  button.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(text);
      button.textContent = words.pr.copied;
    } catch {
      button.textContent = words.course.copyFailed;
    }
  });

async function check(key) {
  const s = stateOf(key);
  Object.assign(s, { step: "checking", error: "" });
  redraw(key);
  try {
    const status = await invoke("github_status");
    s.status = status;
    s.step = status.connected ? "form" : "connect";
  } catch (error) {
    s.error = String(error);
    s.step = "connect";
  }
  redraw(key);
}

async function connect(key) {
  const s = stateOf(key);
  s.error = "";
  try {
    s.device = await invoke("github_connect_start");
    s.step = "waiting";
    redraw(key);
    await wait(key, s.device);
  } catch (error) {
    s.error = String(error);
    s.step = "connect";
    redraw(key);
  }
}

async function wait(key, device) {
  const s = stateOf(key);
  let seconds = Math.max(5, device.interval);
  const until = Date.now() + device.expires_in * 1000;
  while (Date.now() < until && s.step === "waiting") {
    await new Promise((resolve) => setTimeout(resolve, seconds * 1000));
    const progress = await invoke("github_connect_poll", { deviceCode: device.device_code });
    if (progress.status === "granted") {
      s.step = "form";
      return redraw(key);
    }
    if (progress.status === "denied") {
      s.error = progress.message;
      s.step = "connect";
      return redraw(key);
    }
    seconds = Math.max(5, device.interval);
  }
  if (s.step === "waiting") {
    s.error = words.pr.denied;
    s.step = "connect";
    redraw(key);
  }
}

async function send(key, slot) {
  const s = stateOf(key);
  const info = infos.get(key);
  const title = slot.querySelector("[data-title]").value;
  const body = slot.querySelector("[data-body]").value;
  Object.assign(s, { step: "sending", error: "", title, body });
  redraw(key);
  try {
    s.result = await invoke("github_open_pr", { repository: info.repository, branch: info.branch, base: info.base, title, body });
    s.step = "done";
  } catch (error) {
    s.error = String(error);
    s.step = "form";
  }
  redraw(key);
}

function draw(key) {
  const slot = slotOf(key);
  const info = infos.get(key);
  if (!slot || !info) return;
  const s = stateOf(key);
  const t = words.pr;
  const error = s.error ? `<p class="form-error">${escapeHtml(s.error)}</p>` : "";
  let html = "";
  if (s.step === "idle") html = `<button type="button" class="btn" data-act="open">${t.open}</button>`;
  else if (s.step === "checking") html = `<p class="hint">${t.checking}</p>`;
  else if (s.step === "connect") {
    html = `<p class="hint">${t.connectNote}</p>${error}${s.status?.can_connect
      ? `<button type="button" class="btn" data-act="connect">${t.start}</button>`
      : `<p class="hint">${t.ghHint}</p><button type="button" class="btn" data-act="open">${t.checkAgain}</button>`}`;
  } else if (s.step === "waiting") {
    html = `<p class="hint">${t.goTo}</p><div class="cmd"><code>${escapeHtml(s.device.verification_uri)}</code><button type="button" class="btn ghost" data-copy-uri>${t.copy}</button></div>
      <p class="hint">${t.code}</p><div class="cmd"><code class="big-code">${escapeHtml(s.device.user_code)}</code><button type="button" class="btn ghost" data-copy-code>${t.copy}</button></div><p class="hint">${t.waiting}</p>`;
  } else if (s.step === "form" || s.step === "sending") {
    const sending = s.step === "sending";
    html = `<p class="hint">${escapeHtml(t.note(info.branch, info.base))}</p>
      <label class="pick"><span>${t.title}</span><input data-title value="${escapeHtml(s.title ?? info.title)}" ${sending ? "disabled" : ""}></label>
      <label class="pick"><span>${t.body}</span><textarea data-body rows="5" ${sending ? "disabled" : ""}>${escapeHtml(s.body ?? info.body)}</textarea></label>${error}
      <div class="form-foot"><button type="button" class="btn primary" data-act="send" ${sending ? "disabled" : ""}>${sending ? t.sending : t.send}</button>
      ${s.status?.source === "stored" ? `<button type="button" class="btn ghost" data-act="signout">${t.signOut}</button>` : ""}</div>`;
  } else if (s.step === "done") {
    html = `<p class="summary-text">${t.done} #${s.result.number}</p><div class="cmd"><code>${escapeHtml(s.result.url)}</code><button type="button" class="btn ghost" data-copy-url>${t.copy}</button></div>`;
  }
  slot.innerHTML = html;
  slot.querySelector("[data-act=open]")?.addEventListener("click", () => check(key));
  slot.querySelector("[data-act=connect]")?.addEventListener("click", () => connect(key));
  slot.querySelector("[data-act=send]")?.addEventListener("click", () => send(key, slot));
  slot.querySelector("[data-act=signout]")?.addEventListener("click", async () => {
    await invoke("github_disconnect");
    Object.assign(s, { step: "idle", status: null });
    redraw(key);
  });
  const uri = slot.querySelector("[data-copy-uri]");
  if (uri) copy(uri, s.device.verification_uri);
  const code = slot.querySelector("[data-copy-code]");
  if (code) copy(code, s.device.user_code);
  const url = slot.querySelector("[data-copy-url]");
  if (url) copy(url, s.result.url);
}
