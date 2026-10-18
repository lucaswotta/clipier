// Drives the real window with the real backend, then reports through `selftest_report`.
// Started by `CLIPIER_SELFTEST=<file> clipier`; see src/selftest.rs.
(async () => {
  const passed = [];
  const failed = [];
  const errors = [];
  window.addEventListener("error", (e) => errors.push(String(e.message)));
  window.addEventListener("unhandledrejection", (e) => errors.push(String(e.reason)));
  const originalError = console.error;
  console.error = (...args) => {
    errors.push(args.map(String).join(" "));
    originalError(...args);
  };

  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const waitFor = async (fn, ms = 20000) => {
    for (let waited = 0; waited < ms; waited += 100) {
      const value = fn();
      if (value) return value;
      await sleep(100);
    }
    return null;
  };
  const check = (name, ok, detail = "") => (ok ? passed.push(name) : failed.push({ name, detail }));
  const content = () => document.getElementById("content");
  const text = () => content().innerText;
  const menu = (index) => document.querySelectorAll("#menu .item")[index];
  const speak = async (tags) => {
    Object.defineProperty(navigator, "languages", { value: tags, configurable: true });
    Object.defineProperty(navigator, "language", { value: tags[0], configurable: true });
    window.dispatchEvent(new Event("languagechange"));
    await sleep(400);
  };

  try {
    await waitFor(() => menu(2));
    const system = (navigator.languages?.[0] ?? navigator.language ?? "en").toLowerCase();
    const expected = system.startsWith("pt") ? "pt-BR" : system.startsWith("es") ? "es" : "en";
    check("the language starts as the system's, English when it is none of ours", document.documentElement.lang === expected, `${document.documentElement.lang} vs ${navigator.languages}`);
    check("there is no language picker", !document.getElementById("language"));
    await speak(["en-US"]);
    const labels = [...document.querySelectorAll("#menu .item")].map((b) => b.textContent.trim());
    check("the menu names the three screens", ["Chat", "Set course", "Agents"].every((l, i) => labels[i].startsWith(l)), labels.join(" | "));
    check("the brand is Clipier", document.querySelector(".brand")?.textContent.includes("Clipier"));

    // Agents: asked of the real CLIs.
    menu(2).click();
    const cards = await waitFor(() => content().querySelectorAll(".agent-card:not(.skeleton)").length === 4);
    check("the agents screen shows the four CLIs", !!cards, `cards: ${content().querySelectorAll(".agent-card:not(.skeleton)").length} :: ${text().replace(/\s+/g, " ").slice(0, 200)}`);
    check("the roles are listed with who plays them", content().querySelectorAll(".role-card").length >= 4, `roles: ${content().querySelectorAll(".role-card").length}`);
    check("a role has an agent picker and a save button", !!content().querySelector(".role-card [data-field=agent]") && !!content().querySelector(".role-card [data-save]"));

    // Chat.
    menu(0).click();
    const start = await waitFor(() => document.getElementById("chat-repository"));
    check("the chat opens on a repository", !!start && /One conversation, any agent/.test(text()));
    document.getElementById("chat-repository").value = window.__SELFTEST_REPOSITORY__;
    document.getElementById("chat-open").requestSubmit();
    check("starting a conversation shows the composer", !!(await waitFor(() => document.getElementById("chat-form"))));
    check("the composer offers the agents that are ready", document.querySelectorAll("#chat-agent option").length > 0);
    check("the stop button is hidden while nothing runs", document.getElementById("chat-cancel").offsetParent === null);
    check("the chat fits the width", content().scrollWidth <= content().clientWidth);

    // Set course.
    menu(1).click();
    const form = await waitFor(() => document.getElementById("course-form"));
    check("set course shows the form", !!form && !!document.getElementById("course-brief") && !!document.getElementById("course-repository"));
    check("set course explains itself in three steps", document.querySelectorAll(".how li").length === 3);
    check("one boat is ready to go", document.querySelectorAll(".boat-row").length === 1);
    document.getElementById("add-boat").click();
    check("a boat can be added", document.querySelectorAll(".boat-row").length === 2);
    document.querySelector(".boat-row [data-remove]").click();
    check("a boat can be removed", document.querySelectorAll(".boat-row").length === 1);
    document.querySelector("input[name=mode][value=crew]").click();
    await sleep(300);
    check("choosing Crew explains the plan and drops the boats", /captain/i.test(text()) && document.querySelectorAll(".boat-row").length === 0 && document.querySelector("#course-form button[type=submit]").textContent.includes("Draw the plan"));
    document.querySelector("input[name=mode][value=race]").click();
    await sleep(300);
    document.getElementById("course-repository").value = "";
    document.getElementById("course-form").requestSubmit();
    check("sailing without a project says what is missing", !!(await waitFor(() => document.getElementById("course-error")?.textContent.trim())));

    // GitHub: only the status, which asks nothing of the network.
    const github = await window.__TAURI_INTERNALS__.invoke("github_status");
    check("the GitHub status answers", typeof github.connected === "boolean" && ["stored", "gh", "none"].includes(github.source), JSON.stringify(github));

    // Palettes.
    check("the interface uses the light palette", getComputedStyle(document.documentElement).colorScheme === "light");
    document.getElementById("theme-toggle").click();
    check("the night watch toggle switches to the dark palette", document.documentElement.dataset.theme === "dark");
    document.getElementById("theme-toggle").click();
    check("the day watch toggle returns to the light palette", document.documentElement.dataset.theme === "light");
    check("the mark is a button with a name", document.getElementById("mark").tagName === "BUTTON" && !!document.getElementById("mark").getAttribute("aria-label"));

    if (window.__SELFTEST_SAIL__) {
      document.getElementById("course-repository").value = window.__SELFTEST_REPOSITORY__;
      document.getElementById("course-brief").value = window.__SELFTEST_REQUEST__;
      document.getElementById("course-check").value = "";
      document.getElementById("course-form").requestSubmit();
      const arrived = await waitFor(() => document.querySelector(".boat.done, .boat.failed"), 900000);
      check("a boat with a real agent arrives", !!arrived && arrived.classList.contains("done"), arrived ? arrived.innerText.slice(0, 300) : "no boat finished");
      check("the finished boat names the changed file", /hello\.txt/.test(text()), text().slice(0, 300));
      const raced = await window.__TAURI_INTERNALS__.invoke("race_list");
      check("the race was recorded", raced.length === 1, JSON.stringify(raced).slice(0, 200));

      menu(0).click();
      await waitFor(() => document.querySelector("#chat-list .orch-item"));
      document.querySelector("#chat-list .orch-item").click();
      await waitFor(() => document.getElementById("chat-form"));
      document.getElementById("chat-text").value = "Reply with the single word: ready";
      document.getElementById("chat-form").requestSubmit();
      const answered = await waitFor(() => document.querySelector(".msg.agent:not(.working)"), 300000);
      check("a real agent answers in the chat", !!answered && !answered.classList.contains("failed"), answered ? answered.innerText.slice(0, 200) : "no answer");
    }

    if (window.__SELFTEST_CREW__) {
      menu(1).click();
      await waitFor(() => document.getElementById("course-form"));
      document.getElementById("course-new")?.click();
      document.querySelector("input[name=mode][value=crew]").click();
      await sleep(300);
      document.getElementById("course-repository").value = window.__SELFTEST_REPOSITORY__;
      document.getElementById("course-brief").value = window.__SELFTEST_CREW_REQUEST__;
      document.getElementById("course-form").requestSubmit();
      const approve = await waitFor(() => document.getElementById("approve"), 600000);
      check("the captain draws a plan to read before anything starts", !!approve && document.querySelectorAll(".piece").length > 0, text().slice(0, 300));
      if (approve) {
        document.getElementById("approve").click();
        const back = await waitFor(() => document.querySelector(".finish"), 900000);
        check("the crew comes back and the branch holds its work", !!back && /clipier\/[a-z]+-[a-z]+-[0-9a-f]{4}\/integration/.test(text()), text().slice(0, 400));
        check("every piece reached the branch", document.querySelectorAll(".boat.piece-merged").length === document.querySelectorAll(".boats .boat").length, document.querySelector(".boats")?.innerText.slice(0, 300));
      }
    }

    // Languages.
    await speak(["pt-BR", "en"]);
    const portuguese = [...document.querySelectorAll("#menu .item")].map((b) => b.textContent.trim());
    check("the menu speaks Brazilian Portuguese", ["Conversa", "Traçar rota", "Agentes"].every((l, i) => portuguese[i].startsWith(l)), portuguese.join(" | "));
    check("the page itself follows the language", document.documentElement.lang === "pt-BR" && document.getElementById("brand-tagline").textContent === "TODOS OS AGENTES, UM SÓ CAIS.", document.getElementById("brand-tagline").textContent);
    await speak(["es-419"]);
    menu(1).click();
    await sleep(300);
    document.getElementById("again")?.click();
    document.getElementById("course-new")?.click();
    check("a screen redraws in Spanish", !!(await waitFor(() => /Una tarea, varios barcos/.test(text()))), text().slice(0, 100));
    await speak(["fr-FR", "de"]);
    check("a language we do not speak falls back to English", document.documentElement.lang === "en" && [...document.querySelectorAll("#menu .item")].some((b) => b.textContent.includes("Agents")));

    check("the interface raised no errors", errors.length === 0, errors.join(" || "));
  } catch (error) {
    failed.push({ name: "the self test ran to the end", detail: String(error && error.stack ? error.stack : error) });
  }
  await window.__TAURI_INTERNALS__.invoke("selftest_report", { report: JSON.stringify({ passed, failed, errors }) });
})();
