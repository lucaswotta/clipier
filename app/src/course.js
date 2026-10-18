import { invoke } from "@tauri-apps/api/core";
import { icons } from "./icons.js";
import { escapeHtml } from "./text.js";
import { words } from "./vocabulary.js";
import { markdown } from "./chat.js";
import { fleet, loadFleet, modelControl, agentOptions } from "./fleet.js";
import { pullRequestSlot, mountPullRequests } from "./github.js";
import { crewActive, crewRun, crewReset, crewOpen, crewPlan, crewPaint, whenCrewChanges } from "./crew.js";

const REPOSITORY_KEY = "leme-pier.repository";
const MODE_KEY = "leme-pier.course.mode";
const POLL_MS = 800;
const MAX_BOATS = 4;

const form = { mode: "race", repository: "", brief: "", boats: [], check: "", improving: false, undo: null, note: "", error: "" };
const run = { id: null, view: null, diffs: {}, error: "" };
let hosts = null;
let timer = null;
let voyages = [];
whenCrewChanges(() => refreshVoyages());
let onVoyages = () => {};

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
    /* only a convenience */
  }
};

// ---- what the sidebar reads -----------------------------------------------------------------

export const voyageList = () => voyages;
export const activeRuns = () => voyages.filter((v) => v.phase === "running").length;
export const currentRun = () => run.id ?? crewRun();
export function whenVoyagesChange(callback) {
  onVoyages = callback;
}

/** Both kinds of voyage, newest first: races (several boats) and crews (a plan carried out together). */
export async function refreshVoyages() {
  const [races, crews] = await Promise.all([invoke("race_list").catch(() => []), invoke("crew_list").catch(() => [])]);
  voyages = [...races.map((v) => ({ ...v, kind: "race" })), ...crews.map((v) => ({ ...v, kind: "crew" }))].sort((a, b) => b.started_ms - a.started_ms);
  onVoyages();
}

export async function openVoyage(summary) {
  stop();
  crewReset();
  Object.assign(run, { id: null, view: null, diffs: {}, error: "" });
  if (summary.kind === "crew") {
    await crewOpen(summary);
    return;
  }
  Object.assign(run, { id: summary.run });
  await poll();
  if (run.view?.running) start();
}

// ---- following a race --------------------------------------------------------------------

function stop() {
  if (timer) clearInterval(timer);
  timer = null;
}

function start() {
  stop();
  timer = setInterval(poll, POLL_MS);
}

async function poll() {
  if (!run.id) return;
  try {
    const view = await invoke("race_view", { id: run.id });
    const changed = JSON.stringify(view) !== JSON.stringify(run.view);
    run.view = view;
    if (hosts?.content.dataset.view === "new" && changed) paintRun();
    if (!view.running) {
      stop();
      refreshVoyages();
    }
  } catch (error) {
    run.error = String(error);
    stop();
  }
}

// ---- the form -------------------------------------------------------------------------------

function defaultBoat() {
  const agent = fleet.list.find((a) => a.id === stored("leme-pier.chat.agent")) ?? fleet.list[0];
  return { agent: agent?.id ?? "", model: "" };
}

function boatRowHtml(boat, index) {
  const agent = fleet.list.find((a) => a.id === boat.agent);
  return `<div class="boat-row" data-index="${index}">
    <span class="boat-number">${index + 1}</span>
    <label class="pick"><span>${words.chat.agent}</span><select data-field="agent">${agentOptions(fleet.list, boat.agent)}</select></label>
    <label class="pick"><span>${words.chat.model}</span><span data-slot>${modelControl(agent, boat.model, `boat-model-${index}`)}</span></label>
    ${form.boats.length > 1 ? `<button type="button" class="btn ghost" data-remove title="${escapeHtml(words.course.removeBoat)}" aria-label="${escapeHtml(words.course.removeBoat)}">${icons.close(14)}</button>` : ""}
  </div>`;
}

function formHtml() {
  const c = words.course;
  const none = !fleet.list.length;
  return `<form id="course-form" class="course-form" autocomplete="off">
    <header class="page-intro"><span class="eyebrow">${c.eyebrow}</span><h2>${c.heading}</h2><p>${c.explain}</p></header>
    <fieldset class="modes"><legend>${c.modeTitle}</legend>${["race", "crew"].map((mode) => `<label class="mode ${form.mode === mode ? "on" : ""}"><input type="radio" name="mode" value="${mode}" ${form.mode === mode ? "checked" : ""}>
      <strong>${c[mode].title}</strong><span>${c[mode].text}</span></label>`).join("")}</fieldset>
    <ol class="how">${(form.mode === "crew" ? c.crewSteps : c.steps).map((s) => `<li><strong>${s.title}</strong><span>${s.text}</span></li>`).join("")}</ol>

    <section class="step"><h3><span class="n">1</span>${c.project}</h3>
      <input id="course-repository" value="${escapeHtml(form.repository)}" placeholder="/home/you/project" aria-label="${escapeHtml(c.project)}">
      <p class="hint">${c.projectHint}</p></section>

    <section class="step"><h3><span class="n">2</span>${c.brief}</h3>
      <textarea id="course-brief" rows="6" placeholder="${escapeHtml(c.briefPlaceholder)}" aria-label="${escapeHtml(c.brief)}">${escapeHtml(form.brief)}</textarea>
      <div class="brief-actions">
        <button type="button" class="btn" id="course-improve" ${none || form.improving ? "disabled" : ""}>${form.improving ? c.improving : `${icons.refresh(14)}${c.improve}`}</button>
        <button type="button" class="btn ghost" id="course-undo" ${form.undo === null ? "hidden" : ""}>${c.undo}</button>
        <span class="hint" id="course-note">${escapeHtml(form.note) || c.improveHint}</span>
      </div></section>

    ${form.mode === "race" ? `<section class="step"><h3><span class="n">3</span>${c.boats}</h3>
      <p class="hint">${c.boatsHint}</p>
      <div id="boat-rows">${form.boats.map(boatRowHtml).join("")}</div>
      <button type="button" class="btn ghost" id="add-boat" ${form.boats.length >= MAX_BOATS || none ? "disabled" : ""}>${icons.plus(14)}${c.addBoat}</button>
      ${none ? `<p class="form-error">${words.chat.noAgents}</p>` : ""}</section>` : `<section class="step"><p class="hint">${c.crewHint}</p></section>`}

    ${form.mode === "race" ? `<section class="step"><h3><span class="n">4</span>${c.check}</h3>
      <input id="course-check" value="${escapeHtml(form.check)}" placeholder="npm test" aria-label="${escapeHtml(c.check)}">
      <p class="hint">${c.checkHint}</p></section>` : ""}

    <p class="form-error" id="course-error">${escapeHtml(form.error)}</p>
    <div class="form-foot"><button class="btn primary" type="submit" ${none ? "disabled" : ""}>${form.mode === "crew" ? c.drawPlan : c.sail}</button><span class="hint">${c.safe}</span></div>
  </form>`;
}

async function detectCheck() {
  const repository = hosts.content.querySelector("#course-repository")?.value.trim();
  if (!repository?.startsWith("/")) return;
  try {
    const found = await invoke("race_check", { repository });
    const field = hosts.content.querySelector("#course-check");
    if (found && field && !field.value.trim()) {
      field.value = found;
      form.check = found;
    }
  } catch {
    /* not a folder yet */
  }
}

function readForm() {
  const read = (selector) => hosts.content.querySelector(selector)?.value ?? "";
  form.repository = read("#course-repository").trim();
  form.brief = read("#course-brief");
  form.check = read("#course-check").trim();
  hosts.content.querySelectorAll(".boat-row").forEach((row) => {
    const boat = form.boats[Number(row.dataset.index)];
    boat.agent = row.querySelector("[data-field=agent]").value;
    boat.model = row.querySelector("[data-slot] select, [data-slot] input")?.value ?? "";
  });
}

function bindForm() {
  const root = hosts.content;
  for (const radio of root.querySelectorAll("input[name=mode]")) {
    radio.addEventListener("change", () => {
      readForm();
      form.mode = radio.value;
      remember(MODE_KEY, form.mode);
      paintForm();
    });
  }
  root.querySelector("#course-repository").addEventListener("change", () => {
    readForm();
    detectCheck();
  });
  root.querySelector("#boat-rows")?.addEventListener("change", (event) => {
    readForm();
    const row = event.target.closest(".boat-row");
    if (row && event.target.matches("[data-field=agent]")) {
      const boat = form.boats[Number(row.dataset.index)];
      boat.model = "";
      row.querySelector("[data-slot]").innerHTML = modelControl(fleet.list.find((a) => a.id === boat.agent), "", `boat-model-${row.dataset.index}`);
    }
  });
  root.querySelector("#boat-rows")?.addEventListener("click", (event) => {
    const remove = event.target.closest("[data-remove]");
    if (!remove) return;
    readForm();
    form.boats.splice(Number(remove.closest(".boat-row").dataset.index), 1);
    paintForm();
  });
  root.querySelector("#add-boat")?.addEventListener("click", () => {
    readForm();
    form.boats.push(defaultBoat());
    paintForm();
  });
  root.querySelector("#course-improve").addEventListener("click", improve);
  root.querySelector("#course-undo").addEventListener("click", () => {
    form.brief = form.undo ?? form.brief;
    form.undo = null;
    form.note = "";
    paintForm();
  });
  root.querySelector("#course-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    readForm();
    form.error = "";
    remember(REPOSITORY_KEY, form.repository);
    if (form.mode === "crew") {
      if (!form.repository || !form.brief.trim()) {
        form.error = words.course.needBrief;
        paintForm();
        return;
      }
      form.error = await crewPlan({ repository: form.repository, brief: form.brief, repaint: paint });
      if (!crewActive()) paintForm();
      return;
    }
    try {
      const id = await invoke("race_start", { repository: form.repository, brief: form.brief, boats: form.boats, check: form.check });
      await refreshVoyages();
      Object.assign(run, { id, view: null, diffs: {}, error: "" });
      await poll();
      start();
      paint();
    } catch (error) {
      form.error = String(error);
      paintForm();
    }
  });
}

async function improve() {
  readForm();
  if (!form.brief.trim() || !form.repository) {
    form.error = words.course.needBrief;
    paintForm();
    return;
  }
  const first = form.boats[0];
  form.improving = true;
  form.error = "";
  paintForm();
  try {
    const better = await invoke("race_improve", { repository: form.repository, brief: form.brief, agent: first.agent, model: first.model });
    form.undo = form.brief;
    form.brief = better;
    form.note = words.course.improved(first.agent);
  } catch (error) {
    form.error = String(error);
  }
  form.improving = false;
  paintForm();
}

function paintForm() {
  const scroll = hosts.content.scrollTop;
  hosts.content.innerHTML = `<div class="page course-page">${formHtml()}</div>`;
  bindForm();
  hosts.content.scrollTop = scroll;
}

// ---- the race on screen ------------------------------------------------------------------

const compact = (n) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${Math.round(n / 1e3)}k` : String(n));
const seconds = (ms) => (ms >= 60000 ? `${Math.floor(ms / 60000)}m ${Math.round((ms % 60000) / 1000)}s` : `${Math.round(ms / 1000)}s`);

function liveHtml(events) {
  const lines = (events ?? [])
    .slice(-5)
    .map((e) => {
      if (e.kind === "edit") return `<li>${icons.edit(12)}<code>${escapeHtml(e.path)}</code></li>`;
      if (e.kind === "command") return `<li><code>$ ${escapeHtml(e.line.slice(0, 80))}</code></li>`;
      if (e.kind === "tool") return `<li>${escapeHtml(e.name)} ${escapeHtml(e.summary ?? "")}</li>`;
      if (e.kind === "message") return `<li>${escapeHtml(e.text.slice(0, 110))}</li>`;
      return "";
    })
    .join("");
  return lines ? `<ul class="live">${lines}</ul>` : "";
}

function boatCardHtml(boat, race, live) {
  const c = words.course;
  const phase = c.phase[boat.phase];
  const check = boat.check
    ? `<span class="verdict ${boat.check.passed ? "ok" : "bad"}" title="${escapeHtml(boat.check.headline)}">${boat.check.passed ? c.checkPassed : c.checkFailed}</span>`
    : boat.phase === "done" ? `<span class="verdict none">${c.noCheck}</span>` : "";
  const files = boat.files.slice(0, 12).map((p) => `<span class="chip edit">${icons.edit(12)}${escapeHtml(p)}</span>`).join("");
  const more = boat.files.length > 12 ? `<span class="chip">+${boat.files.length - 12}</span>` : "";
  let body = "";
  if (boat.phase === "working" || boat.phase === "checking") body = liveHtml(live) || `<p class="hint">${boat.phase === "checking" ? c.checking : c.working}</p>`;
  else if (boat.summary) body = `<div class="bubble">${markdown(boat.summary)}</div>`;
  const keep = boat.branch
    ? `<details class="keep"><summary>${c.keep}</summary>
        <p class="hint">${c.keepHint}</p>
        <div class="cmd"><code>git -C ${escapeHtml(race.repository)} diff ${escapeHtml(race.base.slice(0, 10))} ${escapeHtml(boat.branch)}</code></div>
        <div class="cmd"><code>git -C ${escapeHtml(race.repository)} merge ${escapeHtml(boat.branch)}</code><button type="button" class="btn ghost" data-copy="git -C ${escapeHtml(race.repository)} merge ${escapeHtml(boat.branch)}">${c.copy}</button></div>
      </details>
      ${race.base_branch ? pullRequestSlot(`boat:${boat.id}`) : ""}
      <details class="changes" data-boat="${escapeHtml(boat.id)}"><summary>${c.seeChanges}</summary><pre class="diff-view">${run.diffs[boat.id] ? escapeHtml(run.diffs[boat.id]) : c.loading}</pre></details>`
    : "";
  return `<article class="boat ${boat.phase}"><header><span class="who">${escapeHtml(boat.agent)} · ${escapeHtml(boat.model || words.chat.defaultModel)}</span><span class="phase ${boat.phase}">${phase}</span></header>
    ${body}
    <div class="boat-facts">${check}${boat.tokens ? `<span class="meta">${compact(boat.tokens)} ${c.tokens}</span>` : ""}${boat.duration_ms ? `<span class="meta">${seconds(boat.duration_ms)}</span>` : ""}</div>
    ${files || more ? `<div class="chips">${files}${more}</div>` : ""}
    ${keep}</article>`;
}

function paintRun() {
  const view = run.view;
  const c = words.course;
  if (!view) {
    hosts.content.innerHTML = `<div class="empty"><p>${escapeHtml(run.error || "…")}</p></div>`;
    return;
  }
  const { race } = view;
  const live = new Map(view.live.map((l) => [l.boat, l.events]));
  const state = race.finished ? c.finished : view.running ? c.underway : c.adrift;
  hosts.topbar.innerHTML = `<h1>${escapeHtml(race.brief.slice(0, 70))}</h1><span class="chat-repo">${escapeHtml(race.repository)}</span>
    ${view.running ? `<button class="btn ghost" id="course-cancel" type="button">${c.cancel}</button>` : ""}<button class="btn" id="course-new" type="button">${icons.plus(14)}${c.newCourse}</button>`;
  hosts.topbar.querySelector("#course-cancel")?.addEventListener("click", () => invoke("race_cancel", { id: run.id }));
  hosts.topbar.querySelector("#course-new").addEventListener("click", newCourse);
  const open = new Set([...hosts.content.querySelectorAll("details[open]")].map((d) => d.dataset.boat ?? d.className));
  hosts.content.innerHTML = `<div class="page course-page">
    <p class="state-line"><span class="phase ${race.finished ? "done" : "working"}">${state}</span>${race.error ? `<span class="form-error">${escapeHtml(race.error)}</span>` : ""}</p>
    <details class="the-brief"><summary>${c.theBrief}</summary><div class="bubble">${markdown(race.brief)}</div></details>
    <div class="boats">${race.boats.map((boat) => boatCardHtml(boat, race, live.get(boat.id))).join("")}</div>
    <p class="hint">${c.safe}</p></div>`;
  const requests = {};
  for (const boat of race.boats) {
    if (!boat.branch || !race.base_branch) continue;
    requests[`boat:${boat.id}`] = {
      repository: race.repository,
      branch: boat.branch,
      base: race.base_branch,
      title: race.brief.replace(/\s+/g, " ").slice(0, 72),
      body: [boat.summary, boat.files.length ? `Files: ${boat.files.join(", ")}` : "", boat.check?.headline ?? ""].filter(Boolean).join("\n\n"),
    };
  }
  mountPullRequests(hosts.content, requests);
  for (const details of hosts.content.querySelectorAll("details[data-boat]")) {
    if (open.has(details.dataset.boat)) details.open = true;
    details.addEventListener("toggle", () => details.open && loadDiff(details));
    if (details.open) loadDiff(details);
  }
  for (const button of hosts.content.querySelectorAll("[data-copy]")) {
    button.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(button.dataset.copy);
        button.textContent = c.copied;
      } catch {
        button.textContent = c.copyFailed;
      }
    });
  }
}

async function loadDiff(details) {
  const boat = details.dataset.boat;
  if (run.diffs[boat]) return;
  try {
    run.diffs[boat] = (await invoke("race_diff", { id: run.id, boat })) || words.course.noChanges;
  } catch (error) {
    run.diffs[boat] = String(error);
  }
  const pre = details.querySelector("pre");
  if (pre) pre.textContent = run.diffs[boat];
}

function newCourse() {
  stop();
  crewReset();
  Object.assign(run, { id: null, view: null, diffs: {}, error: "" });
  paint();
}

function paint() {
  hosts.topbar.innerHTML = `<h1>${words.menu.new}</h1>`;
  if (crewActive()) return crewPaint(hosts, paint);
  if (run.id) return paintRun();
  paintForm();
}

/** The Set course screen redraws itself as its race moves; a new snapshot must not wipe it. */
export async function drawCourse(topbar, content) {
  hosts = { topbar, content };
  await loadFleet();
  // The user may have gone to another screen while the agents were being asked.
  if (content.dataset.view !== "new") return;
  if (!form.boats.length && fleet.list.length) form.boats = [defaultBoat()];
  form.mode = stored(MODE_KEY) === "crew" ? "crew" : "race";
  if (!form.repository) form.repository = stored(REPOSITORY_KEY);
  paint();
  if (run.id) {
    await poll();
    if (run.view?.running) start();
    paintRun();
  } else {
    detectCheck();
  }
}
