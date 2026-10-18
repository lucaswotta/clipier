import "@fontsource-variable/inter";
import "@fontsource/jetbrains-mono/400.css";
import "@fontsource/jetbrains-mono/600.css";
import "./style.css";
import { icons } from "./icons.js";
import { drawAgents } from "./agents.js";
import { drawChat, activeChats } from "./chat.js";
import { drawCourse, activeRuns, voyageList, currentRun, whenVoyagesChange, refreshVoyages, openVoyage } from "./course.js";
import { words } from "./vocabulary.js";
import { applyLanguage, systemLanguage } from "./i18n.js";
import { escapeHtml } from "./text.js";

const menu = document.getElementById("menu");
const voyageNav = document.getElementById("orchestration-list");
const topbar = document.getElementById("topbar");
const content = document.getElementById("content");
const voyages = document.querySelector(".voyages");
const compactNavigation = window.matchMedia("(max-width: 680px)");
voyages.open = !compactNavigation.matches;
compactNavigation.addEventListener("change", () => { voyages.open = !compactNavigation.matches; });
voyageNav.addEventListener("click", (event) => {
  if (compactNavigation.matches && event.target.closest(".orch-item")) voyages.open = false;
});

let view = { kind: "chat" };

/** The text that sits in the page itself rather than in a screen. */
function paintStatic() {
  const set = (id, text) => { document.getElementById(id).textContent = text; };
  set("brand-name", words.brand);
  set("brand-tagline", words.tagline.toUpperCase());
  set("workspace-label", words.ui.workspace);
  set("voyages-label", words.ui.voyages);
  set("workspace-name", words.ui.workspaceName);
  set("workspace-note", words.ui.local);
  document.title = words.brand;
  const mark = document.getElementById("mark");
  mark.setAttribute("aria-label", words.ui.mark);
  mark.title = words.tagline;
  menu.setAttribute("aria-label", words.ui.workspace);
  voyageNav.setAttribute("aria-label", words.ui.voyages);
}

function renderMenu() {
  const badge = (count) => (count ? `<span class="badge">${words.ui.live}</span>` : "");
  const items = [
    { kind: "chat", name: words.menu.chat, icon: icons.chat(16), extra: badge(activeChats()) },
    { kind: "new", name: words.menu.new, icon: icons.plus(16), extra: badge(activeRuns()) },
    { kind: "agents", name: words.menu.agents, icon: icons.settings(16), extra: "" },
  ];
  menu.replaceChildren(
    ...items.map((item) => {
      const button = document.createElement("button");
      button.className = "item";
      button.setAttribute("aria-current", String(view.kind === item.kind));
      button.innerHTML = `${item.icon}<span>${item.name}</span>${item.extra}`;
      button.addEventListener("click", () => openView({ kind: item.kind }));
      return button;
    }),
  );
  const buttons = voyageList().map((v) => {
    const button = document.createElement("button");
    button.className = "orch-item";
    button.setAttribute("aria-current", String(view.kind === "new" && currentRun() === v.run));
    const status = v.phase === "running" ? `<i class="dot running"></i>${words.ui.underway}` : v.phase === "interrupted" ? `<i class="dot failed"></i>${words.ui.adrift}` : v.failed ? `<i class="dot failed"></i>${words.ui.notDone(v.failed)}` : `<i class="dot done"></i>${words.ui.done(v.done)}`;
    button.innerHTML = `${icons.orchestration(16)}<span class="orch-text"><span class="name">${escapeHtml(v.request)}</span><span class="meta">${status}</span></span>`;
    button.addEventListener("click", async () => {
      view = { kind: "new" };
      delete content.dataset.view;
      await openVoyage(v);
      render();
    });
    return button;
  });
  voyageNav.replaceChildren(...buttons);
  document.getElementById("voyage-count").textContent = buttons.length;
  if (!buttons.length) {
    const empty = document.createElement("p");
    empty.className = "sidebar-empty";
    empty.textContent = words.ui.voyagesEmpty;
    voyageNav.append(empty);
  }
}

/** Each screen redraws itself as it moves, so it is only drawn again when the view changes. */
function drawOnce(kind, draw) {
  if (content.dataset.view === kind) return;
  content.dataset.view = kind;
  draw(topbar, content);
}

function openView(next) {
  view = next;
  render();
}

function render() {
  renderMenu();
  if (view.kind === "agents") drawOnce("agents", drawAgents);
  else if (view.kind === "new") drawOnce("new", drawCourse);
  else drawOnce("chat", drawChat);
}

/** A new language redraws everything, including the screen on show. */
function changeLanguage(code) {
  applyLanguage(code);
  paintStatic();
  applyTheme(document.documentElement.dataset.theme);
  delete content.dataset.view;
  render();
}

whenVoyagesChange(renderMenu);
refreshVoyages();
setInterval(() => refreshVoyages(), 4000);

// The flag waves when the mark is clicked.
const mark = document.getElementById("mark");
mark.addEventListener("click", () => {
  mark.classList.remove("waving");
  void mark.getBoundingClientRect();
  mark.classList.add("waving");
});

const THEME_KEY = "clipier.theme";
const themeToggle = document.getElementById("theme-toggle");

function applyTheme(theme) {
  const dark = theme === "dark";
  document.documentElement.dataset.theme = dark ? "dark" : "light";
  const next = dark ? words.theme.light : words.theme.dark;
  themeToggle.innerHTML = dark ? icons.sun(15) : icons.moon(15);
  themeToggle.title = `${next.label} · ${next.hint}`;
  themeToggle.setAttribute("aria-label", next.label);
  themeToggle.setAttribute("aria-pressed", String(dark));
}

// The language is the system's, and follows it if the system changes while the app is open.
applyLanguage(systemLanguage());
paintStatic();
window.addEventListener("languagechange", () => changeLanguage(systemLanguage()));

applyTheme(localStorage.getItem(THEME_KEY) === "dark" ? "dark" : "light");
themeToggle.addEventListener("click", () => {
  const next = document.documentElement.dataset.theme === "dark" ? "light" : "dark";
  localStorage.setItem(THEME_KEY, next);
  applyTheme(next);
});

render();
