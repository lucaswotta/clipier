import { invoke } from "@tauri-apps/api/core";
import { icons } from "./icons.js";
import { formatDuration } from "./format.js";
import { escapeHtml, rich } from "./text.js";
import { words } from "./vocabulary.js";
import { markdown } from "./chat.js";
import { pullRequestSlot, mountPullRequests } from "./github.js";

const POLL_MS = 800;

/** The crew flow on screen: the plan the captain drew, then the crew at work, read back from its record. */
const crew = {
  phase: "form", // form | planning | plan | running
  planning: null,
  plan: null,
  repository: "",
  brief: "",
  error: "",
  id: null,
  view: null,
  diff: "",
};
let host = null;
let timer = null;
let lineTimer = null;
let planningLine = 0;
let onChange = () => {};
let repaintHook = () => {};

export const crewActive = () => crew.phase !== "form";
export const crewRun = () => crew.id;
export function whenCrewChanges(callback) {
  onChange = callback;
}

function stop() {
  if (timer) clearInterval(timer);
  timer = null;
}

export function crewReset() {
  stop();
  clearInterval(lineTimer);
  Object.assign(crew, { phase: "form", planning: null, plan: null, error: "", id: null, view: null, diff: "" });
}

// ---- following a crew -----------------------------------------------------------------------

async function poll() {
  if (!crew.id) return;
  try {
    const view = await invoke("crew_view", { id: crew.id });
    const changed = JSON.stringify(view) !== JSON.stringify(crew.view);
    crew.view = view;
    if (changed && host?.content.dataset.view === "new" && crewActive()) paint();
    if (!view.running) {
      stop();
      onChange();
    }
  } catch (error) {
    crew.error = String(error);
    stop();
  }
}

function follow() {
  stop();
  timer = setInterval(poll, POLL_MS);
}

/** Opens a crew on record. */
export async function crewOpen(summary) {
  crewReset();
  Object.assign(crew, { phase: "running", id: summary.run, repository: summary.repository, brief: summary.request });
  await poll();
  if (crew.view?.running) follow();
}

// ---- asking for the plan, approving it ------------------------------------------------------

export async function crewPlan({ repository, brief, repaint }) {
  repaintHook = repaint;
  Object.assign(crew, { repository, brief, error: "", phase: "planning", planning: crypto.randomUUID() });
  planningLine = 0;
  lineTimer = setInterval(() => {
    planningLine += 1;
    const line = host?.content.querySelector("#planning-line");
    if (line) line.textContent = words.crew.planning.lines[planningLine % words.crew.planning.lines.length];
  }, 2500);
  repaint();
  try {
    crew.plan = await invoke("crew_plan", { requestId: crew.planning, repository, brief });
    crew.phase = "plan";
  } catch (error) {
    crew.phase = "form";
    crew.error = String(error).includes("called off") || String(error).includes("cancelled") ? words.crew.planning.stopped : String(error);
  }
  clearInterval(lineTimer);
  crew.planning = null;
  if (crew.phase === "plan" && !crew.plan.approve_plan) await start(crew.plan.check);
  repaint();
  return crew.error;
}

async function start(check) {
  try {
    crew.id = await invoke("crew_start", { planId: crew.plan.id, check });
    Object.assign(crew, { phase: "running", view: null, diff: "" });
    await poll();
    follow();
    onChange();
  } catch (error) {
    crew.error = String(error);
  }
}

async function approve() {
  await start(host.content.querySelector("#crew-check")?.value ?? "");
  paint();
}

async function discard() {
  crewReset();
  onChange();
  repaintHook();
}

async function decide(command) {
  try {
    await invoke("crew_command", { id: crew.id, command });
  } catch (error) {
    crew.error = String(error);
  }
  await poll();
  paint();
}

// ---- drawing --------------------------------------------------------------------------------

const brief = (text, max = 260) => {
  const flat = String(text).replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max)}…` : flat;
};

const roleName = (role, fallback) => words.agents.roleNames[role]?.title ?? fallback ?? role;

function planningHtml() {
  const c = words.crew.planning;
  return `<div class="empty"><div class="empty-icon">${icons.running(22)}</div><h2>${c.heading}</h2>
    <p id="planning-line">${c.lines[planningLine % c.lines.length]}</p>
    <div class="form-foot"><button class="btn ghost" id="stop-planning" type="button">${c.stop}</button></div></div>`;
}

function pieceHtml(piece, index) {
  const c = words.crew.plan;
  const links = [
    piece.after.length ? `${c.after} ${piece.after.map((d) => `<code>${escapeHtml(d)}</code>`).join(", ")}` : "",
    piece.reviews ? `${c.reviews} <code>${escapeHtml(piece.reviews)}</code>` : "",
  ].filter(Boolean);
  const touches = piece.touches.length ? `<div class="chips"><span class="hint">${c.mayChange}</span>${piece.touches.map((t) => `<code>${escapeHtml(t)}</code>`).join("")}</div>` : "";
  const who = piece.agent ? `${escapeHtml(piece.agent)}${piece.model ? ` · ${escapeHtml(piece.model)}` : ""}` : "—";
  return `<article class="piece"><header><span class="piece-n">${index + 1}</span><strong>${escapeHtml(piece.title)}</strong>
      <span class="hint">${escapeHtml(roleName(piece.role, piece.role_title))} · ${c.doneBy} ${who}</span>
      ${links.length ? `<span class="hint">${links.join(" · ")}</span>` : ""}</header>
    <p>${rich(brief(piece.brief, 420))}</p>${touches}</article>`;
}

function notesHtml() {
  const notes = crew.plan.notes ?? [];
  if (!notes.length) return "";
  const c = words.crew.plan;
  return `<section class="step"><h3>${c.notes} <span class="hint">${notes.length}</span></h3><p class="hint">${c.notesNote}</p>
    <ul class="notes">${notes.map((n) => `<li><div><strong>${escapeHtml(n.title)}</strong> ${rich(n.text)}</div>
      <button type="button" class="icon-btn small" data-forget="${escapeHtml(n.id)}" title="${c.forget}" aria-label="${c.forget}">${icons.trash(14)}</button></li>`).join("")}</ul></section>`;
}

function planHtml(checkValue) {
  const c = words.crew.plan;
  return `<div class="page course-page"><header class="page-intro"><span class="eyebrow">${c.title}</span><h2>${c.heading}</h2><p>${c.intro}</p></header>
    <p class="summary-text">${rich(crew.plan.summary)}</p>
    <div class="pieces">${crew.plan.pieces.map(pieceHtml).join("")}</div>
    ${notesHtml()}
    <section class="step"><h3>${c.check}</h3>
      <input id="crew-check" spellcheck="false" value="${escapeHtml(checkValue ?? crew.plan.check ?? "")}" placeholder="${escapeHtml(c.noCheck)}" aria-label="${escapeHtml(c.check)}">
      <p class="hint">${c.checkNote}</p></section>
    ${crew.error ? `<p class="form-error">${escapeHtml(crew.error)}</p>` : ""}
    <div class="form-foot"><button class="btn ghost" id="discard" type="button">${c.discard}</button><button class="btn primary" id="approve" type="button">${icons.done(16)}${c.approve}</button></div></div>`;
}

function liveHtml(events) {
  const lines = (events ?? [])
    .slice(-4)
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

const PILL = { merged: "done", failed: "failed", conflict: "failed", rejected: "failed", stopped: "failed", working: "working", checking: "working", reviewing: "working" };

function pieceCardHtml(state, live) {
  const c = words.crew;
  const phase = c.phase[state.phase];
  const asks = ["failed", "conflict", "rejected"].includes(state.phase);
  const check = state.check
    ? `<span class="verdict ${state.check.passed ? "ok" : "bad"}" title="${escapeHtml(state.check.headline)}">${state.check.passed ? c.checkPassed : c.checkFailed}</span>`
    : "";
  const files = state.files.slice(0, 10).map((p) => `<span class="chip edit">${icons.edit(12)}${escapeHtml(p)}</span>`).join("");
  const body = ["working", "checking"].includes(state.phase) ? liveHtml(live) : state.summary ? `<div class="bubble">${markdown(state.summary)}</div>` : "";
  return `<article class="boat piece-${state.phase}"><header><span class="who">${escapeHtml(state.piece.title)}</span>
      <span class="phase ${PILL[state.phase] ?? ""}" title="${escapeHtml(phase.hint)}">${phase.label}</span></header>
    <p class="hint">${escapeHtml(roleName(state.piece.role))} · ${escapeHtml(state.agent)}${state.model ? ` · ${escapeHtml(state.model)}` : ""}${state.attempts > 1 ? ` · ×${state.attempts}` : ""}</p>
    ${body}
    <div class="boat-facts">${check}${state.tokens ? `<span class="meta">${compact(state.tokens)} ${words.course.tokens}</span>` : ""}${state.duration_ms ? `<span class="meta">${formatDuration(state.duration_ms / 1000)}</span>` : ""}</div>
    ${files ? `<div class="chips">${files}</div>` : ""}
    ${asks ? `<div class="decision"><p>${c.run.needsYou}</p><div class="decision-actions"><button type="button" class="btn ghost" data-decide="redo" data-piece="${escapeHtml(state.piece.id)}">${c.actions.redo}</button><button type="button" class="btn ghost" data-decide="skip" data-piece="${escapeHtml(state.piece.id)}">${c.actions.skip}</button></div></div>` : ""}
  </article>`;
}

const compact = (n) => (n >= 1e6 ? `${(n / 1e6).toFixed(1)}M` : n >= 1e3 ? `${Math.round(n / 1e3)}k` : String(n));

function diffHtml(patch) {
  const lines = patch.split("\n").slice(0, 600).map((line) => {
    const kind = line.startsWith("@@") ? "hunk" : line.startsWith("+") && !line.startsWith("+++") ? "add" : line.startsWith("-") && !line.startsWith("---") ? "del" : "";
    return `<span class="${kind}">${escapeHtml(line) || " "}</span>`;
  });
  return `<pre class="diff-view">${lines.join("")}</pre>`;
}

function finishedHtml(view) {
  const c = words.crew.finished;
  const state = view.crew;
  const branch = escapeHtml(state.integration);
  const merged = state.pieces.some((p) => p.phase === "merged" && p.files.length);
  const repository = escapeHtml(state.repository);
  return `<section class="finish"><h3 class="heading">${c.title}</h3>
    ${state.checkout_untouched === false ? `<p class="form-error">${c.touched}</p>` : ""}
    ${merged ? `<p class="summary-text">${c.cargo} <code>${branch}</code>. ${c.untouched}</p>
      <div class="cmd"><code>git -C ${repository} diff ${escapeHtml(state.base.slice(0, 10))} ${branch}</code></div>
      <div class="cmd"><code>git -C ${repository} merge ${branch}</code><button type="button" class="btn ghost" data-copy="git -C ${repository} merge ${branch}">${words.course.copy}</button></div>
      ${state.base_branch ? pullRequestSlot("crew") : ""}
      <details class="changes"><summary>${words.course.seeChanges}</summary>${crew.diff ? diffHtml(crew.diff) : `<pre class="diff-view">${words.course.loading}</pre>`}</details>` : `<p class="summary-text">${c.nothing}</p>`}
    <div class="form-foot"><button class="btn primary" id="again" type="button">${icons.plus(16)}${c.again}</button></div></section>`;
}

function runHtml() {
  const view = crew.view;
  const c = words.crew;
  if (!view) return `<div class="empty"><p>${escapeHtml(crew.error || "…")}</p></div>`;
  const live = new Map(view.live.map((l) => [l.piece, l.events]));
  const state = view.crew;
  const adrift = !state.finished && !view.running;
  return `<div class="page course-page">
    <details class="the-brief" open><summary>${c.run.brief}</summary><div class="bubble">${markdown(state.brief)}</div>${state.plan_summary ? `<p class="hint">${escapeHtml(state.plan_summary)}</p>` : ""}</details>
    ${state.error ? `<p class="form-error">${escapeHtml(state.error)}</p>` : ""}
    <div class="boats">${state.pieces.map((p) => pieceCardHtml(p, live.get(p.piece.id))).join("")}</div>
    ${state.finished ? finishedHtml(view) : adrift ? `<section class="finish"><h3 class="heading">${c.finished.adrift}</h3><p class="summary-text">${c.finished.adriftNote} <code>${escapeHtml(state.integration)}</code>.</p><div class="form-foot"><button class="btn primary" id="again" type="button">${icons.plus(16)}${c.finished.again}</button></div></section>` : `<div class="form-foot"><button class="btn ghost" id="stop-all" type="button">${c.run.stopAll}</button></div>`}
  </div>`;
}

function titleOf() {
  const c = words.crew;
  if (crew.phase === "planning") return c.planning.heading;
  if (crew.phase === "plan") return c.plan.title;
  return c.run.title;
}

function paint() {
  const hosts = host;
  hosts.topbar.innerHTML = `<h1>${escapeHtml(titleOf())}</h1>${crew.repository ? `<span class="chat-repo">${escapeHtml(crew.repository)}</span>` : ""}`;
  const checkValue = hosts.content.querySelector("#crew-check")?.value;
  const scroll = hosts.content.scrollTop;
  const open = new Set([...hosts.content.querySelectorAll("details[open]")].map((d) => d.className));
  hosts.content.innerHTML = { planning: planningHtml, plan: () => planHtml(checkValue), running: runHtml }[crew.phase]();
  for (const details of hosts.content.querySelectorAll("details")) if (details.className && open.has(details.className)) details.open = true;
  hosts.content.scrollTop = scroll;
  bind();
  const state = crew.view?.crew;
  if (state?.finished && state.base_branch) {
    const merged = state.pieces.filter((p) => p.phase === "merged").map((p) => `- ${p.piece.title}`);
    mountPullRequests(hosts.content, {
      crew: {
        repository: state.repository,
        branch: state.integration,
        base: state.base_branch,
        title: state.brief.replace(/\s+/g, " ").slice(0, 72),
        body: [state.plan_summary, merged.length ? merged.join("\n") : ""].filter(Boolean).join("\n\n"),
      },
    });
  }
  const diff = hosts.content.querySelector(".changes");
  if (diff) {
    diff.addEventListener("toggle", async () => {
      if (!diff.open || crew.diff) return;
      try {
        crew.diff = (await invoke("crew_diff", { id: crew.id })) || words.course.noChanges;
      } catch (error) {
        crew.diff = String(error);
      }
      paint();
    });
  }
}

function bind() {
  const on = (selector, handler) => host.content.querySelector(selector)?.addEventListener("click", handler);
  on("#stop-planning", () => crew.planning && invoke("crew_stop_planning", { requestId: crew.planning }));
  on("#approve", approve);
  on("#discard", discard);
  on("#stop-all", () => decide({ kind: "stop" }));
  on("#again", async () => {
    crewReset();
    onChange();
    repaintHook();
  });
  for (const button of host.content.querySelectorAll("[data-forget]")) {
    button.addEventListener("click", async () => {
      try {
        await invoke("note_forget", { repository: crew.repository, id: button.dataset.forget });
        crew.plan.notes = crew.plan.notes.filter((n) => n.id !== button.dataset.forget);
      } catch (error) {
        crew.error = String(error);
      }
      paint();
    });
  }
  for (const button of host.content.querySelectorAll("[data-decide]")) {
    button.addEventListener("click", () => decide({ kind: button.dataset.decide, piece: button.dataset.piece }));
  }
  for (const button of host.content.querySelectorAll("[data-copy]")) {
    button.addEventListener("click", async () => {
      try {
        await navigator.clipboard.writeText(button.dataset.copy);
        button.textContent = words.course.copied;
      } catch {
        button.textContent = words.course.copyFailed;
      }
    });
  }
}

/** Draws whatever the crew flow shows now. */
export function crewPaint(hosts, repaint) {
  host = hosts;
  repaintHook = repaint;
  paint();
}
