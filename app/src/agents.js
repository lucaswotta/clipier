import { invoke } from "@tauri-apps/api/core";
import { icons } from "./icons.js";
import { formatDuration } from "./format.js";
import { escapeHtml } from "./text.js";
import { words } from "./vocabulary.js";
import { fleet, modelControl } from "./fleet.js";

const REPOSITORY_KEY = "leme-pier.repository";
const NAMES = { claude: "Claude Code", codex: "Codex", agy: "Antigravity", opencode: "OpenCode" };
const VISIBLE_MODELS = 5;

let data = null;
let loading = false;
let loadError = null;
let hosts = null;
let saved = null;

const compact = (n) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${Math.round(n / 1e3)}k` : String(n));

function repository() {
  try {
    return localStorage.getItem(REPOSITORY_KEY) ?? "";
  } catch {
    return "";
  }
}

async function load(force) {
  if (loading) return;
  loading = true;
  if (force) data = null;
  loadError = null;
  paint();
  try {
    fleet.list = null;
    const [agents, usage, roles] = await Promise.all([
      invoke("detect_agents"),
      invoke("usage_summary"),
      invoke("roles_list", { repository: repository() || null }).catch(() => ({ roles: [], problems: [] })),
    ]);
    fleet.list = agents.filter((a) => a.installed && a.signed_in !== false).map((a) => ({ ...a, id: a.agent }));
    data = { agents, usage: new Map(usage.map((u) => [u.agent, u])), roles: roles.roles, problems: roles.problems };
  } catch (error) {
    loadError = String(error);
  }
  loading = false;
  if (hosts?.content.dataset.view === "agents") paint();
}

function status(agent) {
  const t = words.agents;
  if (!agent.installed) return ["failed", t.notInstalled];
  if (agent.signed_in === false) return ["failed", t.loggedOut];
  return ["done", t.ready];
}

function allowanceHtml(allowance) {
  const t = words.agents;
  const row = (label, window) =>
    window
      ? `<div class="meter"><span class="meter-label">${label}</span><span class="meter-bar"><i style="width:${Math.min(100, window.percent)}%"></i></span><span class="meter-value">${window.percent}% ${t.used}</span><span class="hint">${t.resets} ${escapeHtml(window.resets)}</span></div>`
      : "";
  return `<div class="agent-block"><span class="label">${t.allowance}</span>${row(t.session, allowance.session)}${row(t.week, allowance.week)}</div>`;
}

function agentCard(agent) {
  const t = words.agents;
  const [tone, label] = status(agent);
  const usage = data.usage.get(agent.agent);
  const shown = agent.models.slice(0, VISIBLE_MODELS);
  const rest = agent.models.length - shown.length;
  const name = NAMES[agent.agent] ?? agent.agent;
  return `<article class="agent-card ${agent.installed && agent.signed_in !== false ? "" : "inactive"}">
    <div class="agent-head"><span class="avatar avatar-${agent.agent}">${name[0]}</span>
      <div class="agent-title"><span class="agent-name">${name}</span><span class="sub"><code>${agent.agent}</code> · ${escapeHtml(agent.version ?? "—")}</span></div>
      <span class="status-pill ${tone}"><i class="dot ${tone}"></i>${label}</span></div>
    <dl class="facts-grid"><div><dt>${t.account}</dt><dd>${escapeHtml(agent.account ?? "—")}</dd></div>
      <div><dt>${t.models}</dt><dd>${agent.models.length ? t.modelsCount(agent.models.length) : t.noModels}</dd></div></dl>
    ${agent.allowance ? allowanceHtml(agent.allowance) : ""}
    <div class="agent-block"><span class="label">${t.usage}</span>${usage
      ? `<div class="metrics"><div><span class="value">${usage.uses}</span><span class="label">${t.uses}</span></div>
          <div><span class="value">${usage.delivered}</span><span class="label">${t.delivered}</span></div>
          <div><span class="value">${usage.average_ms === null ? "—" : formatDuration(usage.average_ms / 1000)}</span><span class="label">${t.average}</span></div>
          <div><span class="value">${compact(usage.tokens)}</span><span class="label">${t.tokens}</span></div></div>`
      : `<span class="muted">${t.nothingYet}</span>`}</div>
    ${shown.length ? `<div class="agent-block"><div class="chips">${shown.map((m) => `<code class="model-chip">${escapeHtml(m.id)}</code>`).join("")}${rest > 0 ? `<span class="muted">+${rest}</span>` : ""}</div></div>` : ""}
  </article>`;
}

function roleCard(entry) {
  const t = words.agents.roles;
  const known = words.agents.roleNames[entry.name];
  const usable = fleet.list ?? [];
  const agentOptions = [`<option value="">${t.firstAvailable}</option>`, ...usable.map((a) => `<option value="${a.id}" ${entry.agent === a.id ? "selected" : ""}>${NAMES[a.id] ?? a.id}</option>`)].join("");
  const fallbackOptions = [`<option value="">${t.none}</option>`, ...usable.map((a) => `<option value="${a.id}" ${entry.fallback?.agent === a.id ? "selected" : ""}>${NAMES[a.id] ?? a.id}</option>`)].join("");
  const chosen = usable.find((a) => a.id === entry.agent);
  const fallback = usable.find((a) => a.id === entry.fallback?.agent);
  const unavailable = !entry.assigned;
  return `<article class="role-card" data-role="${escapeHtml(entry.name)}">
    <div class="role-head"><div class="role-title"><span class="role-name">${escapeHtml(known?.title ?? entry.title)}</span>
        <span class="sub">${escapeHtml(known?.description ?? entry.prompt.split("\n")[0])}</span></div>
      <span class="chip">${entry.mode === "write" ? t.writes : t.reads}</span></div>
    <div class="role-pick">
      <label class="pick"><span>${t.agent}</span><select data-field="agent">${agentOptions}</select></label>
      <label class="pick"><span>${t.model}</span><span data-slot="model">${modelControl(chosen, entry.model, `model-${entry.name}`)}</span></label>
      <label class="pick"><span>${t.fallback}</span><select data-field="fallback_agent">${fallbackOptions}</select></label>
      <label class="pick"><span>${t.model}</span><span data-slot="fallback">${modelControl(fallback, entry.fallback?.model ?? "", `fallback-${entry.name}`)}</span></label>
      <button class="btn" type="button" data-save="${escapeHtml(entry.name)}">${saved === entry.name ? t.saved : t.save}</button>
    </div>
    ${unavailable ? `<p class="form-error">${t.unavailable}</p>` : `<p class="hint">${escapeHtml(NAMES[entry.assigned.agent] ?? entry.assigned.agent)}${entry.assigned.model ? ` · ${escapeHtml(entry.assigned.model)}` : ""}${entry.assigned.used_fallback ? " (fallback)" : ""}</p>`}
  </article>`;
}

function paint() {
  const t = words.agents;
  hosts.topbar.innerHTML = `<h1>${t.title}</h1><button class="btn" id="rescan" type="button">${icons.refresh(15)}<span>${t.scan}</span></button>`;
  hosts.topbar.querySelector("#rescan").addEventListener("click", () => load(true));
  if (loadError) {
    hosts.content.innerHTML = `<div class="empty"><p>${escapeHtml(loadError)}</p></div>`;
    return;
  }
  if (!data) {
    hosts.content.innerHTML = `<div class="page"><h3 class="heading">${t.title}</h3><div class="agent-grid">${[0, 1, 2, 3].map(() => `<div class="agent-card skeleton"></div>`).join("")}</div><p class="muted">${t.scanning}</p></div>`;
    return;
  }
  hosts.content.innerHTML = `<div class="page"><header class="page-intro"><span class="eyebrow">${t.eyebrow}</span><h2>${t.heading}</h2><p>${t.introduction}</p></header>
    <div class="agent-grid">${data.agents.map(agentCard).join("")}</div>
    <h3 class="heading">${t.roles.title}</h3><p class="hint">${t.roles.introduction} ${t.roles.projectNote}</p>
    ${data.problems.length ? `<div class="form-error"><strong>${t.roles.problems}</strong><ul>${data.problems.map((p) => `<li>${escapeHtml(p)}</li>`).join("")}</ul></div>` : ""}
    <div class="role-grid">${data.roles.map(roleCard).join("")}</div></div>`;
  bindRoles();
}

function bindRoles() {
  for (const card of hosts.content.querySelectorAll(".role-card")) {
    const name = card.dataset.role;
    const field = (selector) => card.querySelector(selector)?.value ?? "";
    for (const [select, slot, prefix] of [["agent", "model", "model"], ["fallback_agent", "fallback", "fallback"]]) {
      card.querySelector(`[data-field=${select}]`).addEventListener("change", (event) => {
        const agent = (fleet.list ?? []).find((a) => a.id === event.target.value);
        card.querySelector(`[data-slot=${slot}]`).innerHTML = modelControl(agent, "", `${prefix}-${name}`);
      });
    }
    card.querySelector("[data-save]").addEventListener("click", async () => {
      try {
        await invoke("role_save", { name, agent: field("[data-field=agent]"), model: field(`#model-${name}`), fallbackAgent: field("[data-field=fallback_agent]"), fallbackModel: field(`#fallback-${name}`) });
        saved = name;
        await load(false);
      } catch (error) {
        loadError = String(error);
        paint();
      }
    });
  }
}

/** The Agents screen redraws only on request or when detection ends, not every second. */
export async function drawAgents(topbar, content) {
  hosts = { topbar, content };
  if (!data && !loading) await load(false);
  else paint();
}
