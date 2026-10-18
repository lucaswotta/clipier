import { invoke } from "@tauri-apps/api/core";
import { escapeHtml } from "./text.js";
import { words } from "./vocabulary.js";

/** The agents installed and signed in on this machine, read once. */
export const fleet = { list: null };

export async function loadFleet() {
  if (fleet.list) return;
  try {
    const found = await invoke("detect_agents");
    fleet.list = found.filter((a) => a.installed && a.signed_in !== false).map((a) => ({ ...a, id: a.agent }));
  } catch (error) {
    fleet.list = [];
  }
}

/** A list of the models the CLI reports, or a free field when it cannot list them; empty means the CLI's own default. */
export function modelControl(agent, chosen, id = "chat-model") {
  if (!agent?.models.length) return `<input id="${id}" value="${escapeHtml(chosen ?? "")}" placeholder="${escapeHtml(words.chat.defaultModel)}">`;
  const options = [{ id: "", name: words.chat.defaultModel }, ...agent.models];
  return `<select id="${id}">${options.map((m) => `<option value="${escapeHtml(m.id)}" ${m.id === chosen ? "selected" : ""}>${escapeHtml(m.name || m.id)}</option>`).join("")}</select>`;
}

export const agentOptions = (agents, chosen) =>
  agents.map((a) => `<option value="${escapeHtml(a.id)}" ${a.id === chosen ? "selected" : ""}>${escapeHtml(a.id)}</option>`).join("");
