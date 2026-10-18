import { invoke } from "@tauri-apps/api/core";
import { icons } from "./icons.js";
import { escapeHtml } from "./text.js";
import { words } from "./vocabulary.js";
import { fleet, loadFleet, modelControl, agentOptions } from "./fleet.js";

const REPOSITORY_KEY = "clipier.repository";
const AGENT_KEY = "clipier.chat.agent";
const MODEL_KEY = "clipier.chat.model";
const POLL_MS = 700;

const state = {
  conversations: [],
  id: null,
  meta: null,
  entries: [],
  total: 0,
  working: null,
  error: null,
};
let hosts = null;
let timer = null;

const stored = (key) => {
  try {
    return localStorage.getItem(key) ?? "";
  } catch {
    return "";
  }
};
const remember = (key, value) => {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* the choice is only a convenience */
  }
};

export const activeChats = () => state.conversations.filter((c) => c.busy).length;

/** Escaped text with fenced code, inline code, bold and bullet lists. Nothing else is interpreted. */
export function markdown(text) {
  const blocks = String(text ?? "").split(/```[^\n]*\n?/);
  return blocks
    .map((block, index) => {
      if (index % 2) return `<pre><code>${escapeHtml(block.replace(/\n$/, ""))}</code></pre>`;
      const html = [];
      let list = false;
      for (const line of block.split("\n")) {
        const item = /^\s*[-*]\s+(.*)$/.exec(line);
        if (item && !list) html.push("<ul>");
        if (!item && list) html.push("</ul>");
        list = Boolean(item);
        const inline = (s) => escapeHtml(s).replace(/`([^`]+)`/g, "<code>$1</code>").replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>");
        if (item) html.push(`<li>${inline(item[1])}</li>`);
        else if (line.trim()) html.push(`<p>${inline(line)}</p>`);
      }
      if (list) html.push("</ul>");
      return html.join("");
    })
    .join("");
}

const compact = (n) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${Math.round(n / 1e3)}k` : String(n));
const seconds = (ms) => (ms >= 60000 ? `${Math.floor(ms / 60000)}m ${Math.round((ms % 60000) / 1000)}s` : `${Math.round(ms / 1000)}s`);

export async function refreshChats() {
  try {
    state.conversations = await invoke("chat_list");
  } catch (error) {
    state.conversations = [];
  }
}

function stop() {
  if (timer) clearInterval(timer);
  timer = null;
}

async function open(id) {
  Object.assign(state, { id, meta: null, entries: [], total: 0, working: null, error: null });
  await poll();
  if (!timer) timer = setInterval(poll, POLL_MS);
  paint();
}

async function poll() {
  if (hosts?.content.dataset.view !== "chat" || !state.id) {
    stop();
    return;
  }
  try {
    const view = await invoke("chat_view", { id: state.id, since: state.total });
    const grew = view.entries.length > 0;
    const was = JSON.stringify(state.working);
    state.meta = view.meta;
    state.entries.push(...view.entries);
    state.total = view.total;
    state.working = view.working;
    if (grew || was !== JSON.stringify(state.working)) paintMessages();
    if (grew) refreshChats().then(paintList);
    paintComposer();
  } catch (error) {
    state.error = String(error);
    stop();
    paint();
  }
}

// ---- drawing --------------------------------------------------------------------------------

function agentLabel(agent, model) {
  return `${escapeHtml(agent)} · ${escapeHtml(model)}`;
}

function entryHtml(entry, previous) {
  if (entry.kind === "user") return `<article class="msg user"><div class="bubble">${markdown(entry.text)}</div></article>`;
  const handover = previous && previous.agent && previous.agent !== entry.agent
    ? `<div class="handover">${icons.refresh(13)}<span>${escapeHtml(words.chat.handover(previous.agent, entry.agent))}</span></div>`
    : "";
  if (entry.kind === "failed") {
    return `${handover}<article class="msg agent failed"><header>${agentLabel(entry.agent, entry.model)}</header><div class="bubble"><p>${escapeHtml(entry.detail)}</p></div></article>`;
  }
  const chips = [
    ...entry.edits.map((p) => `<span class="chip edit" title="${words.chat.edited}">${icons.edit(12)}${escapeHtml(p)}</span>`),
    ...entry.commands.map((c) => `<span class="chip cmd" title="${words.chat.ran}">$ ${escapeHtml(c.length > 60 ? `${c.slice(0, 59)}…` : c)}</span>`),
  ].join("");
  return `${handover}<article class="msg agent"><header>${agentLabel(entry.agent, entry.model)}<span class="meta">${compact(entry.tokens)} ${words.chat.tokens} · ${seconds(entry.duration_ms)}</span></header>
    <div class="bubble">${markdown(entry.text)}</div>${chips ? `<div class="chips">${chips}</div>` : ""}</article>`;
}

function workingHtml() {
  const w = state.working;
  if (!w) return "";
  const lines = w.events
    .slice(-6)
    .map((e) => {
      if (e.kind === "edit") return `<li>${icons.edit(12)}<code>${escapeHtml(e.path)}</code></li>`;
      if (e.kind === "command") return `<li><code>$ ${escapeHtml(e.line.slice(0, 90))}</code></li>`;
      if (e.kind === "tool") return `<li>${escapeHtml(e.name)} ${escapeHtml(e.summary ?? "")}</li>`;
      if (e.kind === "message") return `<li>${escapeHtml(e.text.slice(0, 120))}</li>`;
      return "";
    })
    .join("");
  return `<article class="msg agent working"><header>${agentLabel(w.agent, w.model)}<span class="meta"><i class="dot running"></i>${words.chat.working}</span></header><ul class="live">${lines}</ul></article>`;
}

function paintMessages() {
  const box = hosts.content.querySelector("#chat-messages");
  if (!box) return;
  const nearBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 80;
  let previousAgent = null;
  const html = state.entries
    .map((entry) => {
      const out = entryHtml(entry, previousAgent);
      if (entry.kind !== "user") previousAgent = entry;
      return out;
    })
    .join("");
  box.innerHTML = (html || `<div class="empty chat-empty"><h2>${words.chat.emptyTitle}</h2><p>${words.chat.emptyNote}</p></div>`) + workingHtml();
  if (nearBottom) box.scrollTop = box.scrollHeight;
}

function paintList() {
  const list = hosts?.content.querySelector("#chat-list");
  if (!list) return;
  list.replaceChildren(
    ...state.conversations.map((c) => {
      const button = document.createElement("button");
      button.className = "orch-item";
      button.setAttribute("aria-current", String(c.id === state.id));
      const repo = c.repository.split("/").filter(Boolean).pop() ?? c.repository;
      button.innerHTML = `<span class="orch-text"><span class="name">${escapeHtml(c.title)}</span><span class="meta">${c.busy ? '<i class="dot running"></i>' : ""}${escapeHtml(repo)}</span></span>`;
      button.addEventListener("click", () => open(c.id));
      return button;
    }),
  );
}

function paintComposer() {
  const form = hosts.content.querySelector("#chat-form");
  if (!form) return;
  const busy = Boolean(state.working);
  form.querySelector("#chat-send").hidden = busy;
  form.querySelector("#chat-cancel").hidden = !busy;
  form.querySelector("#chat-text").disabled = false;
}

function composerHtml() {
  const agents = fleet.list ?? [];
  const agent = agents.find((a) => a.id === stored(AGENT_KEY)) ?? agents[0];
  const model = stored(MODEL_KEY);
  return `<form id="chat-form" autocomplete="off">
    <textarea id="chat-text" rows="3" placeholder="${escapeHtml(words.chat.placeholder)}" aria-label="${escapeHtml(words.chat.placeholder)}"></textarea>
    <div class="composer-row">
      <label class="pick"><span>${words.chat.agent}</span><select id="chat-agent" ${agents.length ? "" : "disabled"}>${agentOptions(agents, agent?.id)}</select></label>
      <label class="pick"><span>${words.chat.model}</span><span id="chat-model-slot">${modelControl(agent, model)}</span></label>
      <span class="composer-note">${agents.length ? words.chat.switchNote : words.chat.noAgents}</span>
      <button class="btn ghost" id="chat-cancel" type="button" hidden>${words.chat.cancel}</button>
      <button class="btn primary" id="chat-send" type="submit" ${agents.length ? "" : "disabled"}>${words.chat.send}</button>
    </div></form>`;
}

function bindComposer() {
  const form = hosts.content.querySelector("#chat-form");
  const agentSelect = form.querySelector("#chat-agent");
  const slot = form.querySelector("#chat-model-slot");
  const modelValue = () => form.querySelector("#chat-model")?.value ?? "";
  const text = form.querySelector("#chat-text");
  agentSelect.addEventListener("change", () => {
    remember(AGENT_KEY, agentSelect.value);
    slot.innerHTML = modelControl(fleet.list.find((a) => a.id === agentSelect.value), "");
    remember(MODEL_KEY, modelValue());
  });
  slot.addEventListener("change", () => remember(MODEL_KEY, modelValue()));
  text.addEventListener("keydown", (event) => {
    if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) form.requestSubmit();
  });
  form.querySelector("#chat-cancel").addEventListener("click", () => invoke("chat_cancel", { id: state.id }));
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const message = text.value.trim();
    if (!message || state.working) return;
    remember(AGENT_KEY, agentSelect.value);
    remember(MODEL_KEY, modelValue());
    try {
      await invoke("chat_send", { id: state.id, text: message, agent: agentSelect.value, model: modelValue() });
      text.value = "";
      state.error = null;
      await poll();
      refreshChats().then(paintList);
    } catch (error) {
      state.error = String(error);
      paintError();
    }
  });
}

function paintError() {
  const box = hosts.content.querySelector("#chat-error");
  if (box) box.textContent = state.error ?? "";
}

async function create(repository) {
  try {
    const id = await invoke("chat_create", { repository });
    remember(REPOSITORY_KEY, repository);
    await refreshChats();
    await open(id);
  } catch (error) {
    state.error = String(error);
    paint();
  }
}

function paintTopbar() {
  const repo = state.meta?.repository ?? "";
  hosts.topbar.innerHTML = `<h1>${escapeHtml(state.meta?.title ?? words.menu.chat)}</h1><span class="chat-repo">${escapeHtml(repo)}</span>
    <button class="btn" id="chat-new" type="button">${icons.plus(14)}${words.chat.new}</button>`;
  hosts.topbar.querySelector("#chat-new").addEventListener("click", () => {
    stop();
    Object.assign(state, { id: null, meta: null, entries: [], total: 0, working: null, error: null });
    paint();
  });
}

function startHtml() {
  return `<div class="chat-start"><div class="empty"><div class="empty-icon">${icons.orchestration(22)}</div><h2>${words.chat.startTitle}</h2><p>${words.chat.startNote}</p></div>
    <form id="chat-open" class="chat-open"><label class="field"><span class="label">${words.chat.repository}</span><input id="chat-repository" value="${escapeHtml(stored(REPOSITORY_KEY))}" placeholder="/home/you/project" required></label>
    <p class="form-error" id="chat-error">${escapeHtml(state.error ?? "")}</p>
    <button class="btn primary" type="submit">${words.chat.start}</button></form></div>`;
}

function paint() {
  hosts.topbar.innerHTML = `<h1>${words.menu.chat}</h1>`;
  const list = `<nav id="chat-list" class="chat-list" aria-label="${words.chat.conversations}"></nav>`;
  if (!state.id) {
    paintTopbarBare();
    hosts.content.innerHTML = `<div class="chat-layout">${list}<section class="chat-main">${startHtml()}</section></div>`;
    hosts.content.querySelector("#chat-open").addEventListener("submit", (event) => {
      event.preventDefault();
      create(hosts.content.querySelector("#chat-repository").value.trim());
    });
    paintList();
    return;
  }
  paintTopbar();
  hosts.content.innerHTML = `<div class="chat-layout">${list}<section class="chat-main"><div id="chat-messages" class="chat-messages"></div><p class="form-error" id="chat-error">${escapeHtml(state.error ?? "")}</p>${composerHtml()}</section></div>`;
  bindComposer();
  paintMessages();
  paintList();
  paintComposer();
  const box = hosts.content.querySelector("#chat-messages");
  box.scrollTop = box.scrollHeight;
}

function paintTopbarBare() {
  hosts.topbar.innerHTML = `<h1>${words.menu.chat}</h1>`;
}

/** The Chat screen redraws itself as its conversation moves; a new snapshot must not wipe it. */
export async function drawChat(topbar, content) {
  hosts = { topbar, content };
  await Promise.all([loadFleet(), refreshChats()]);
  // The user may have gone to another screen while the agents were being asked.
  if (content.dataset.view !== "chat") return;
  paint();
  if (state.id) {
    if (!timer) timer = setInterval(poll, POLL_MS);
    await poll();
  }
}
