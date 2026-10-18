import { words } from "./vocabulary.js";
import ptBR from "./locales/pt-BR.js";
import es from "./locales/es.js";

export const LANGUAGES = [
  { code: "en", label: "English" },
  { code: "pt-BR", label: "Português" },
  { code: "es", label: "Español" },
];
const catalogs = { "pt-BR": ptBR, es };

const copy = (value) => {
  if (Array.isArray(value)) return value.map(copy);
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([key, inner]) => [key, copy(inner)]));
  return value;
};
const english = copy(words);

/** Writes `source` over `target` in place, so every module that imported `words` sees the change. */
function overwrite(target, source) {
  for (const [key, value] of Object.entries(source)) {
    if (value && typeof value === "object" && !Array.isArray(value) && target[key] && typeof target[key] === "object") overwrite(target[key], value);
    else target[key] = copy(value);
  }
}

/** The first of the system's languages that Leme Pier speaks, else English. */
export function systemLanguage() {
  for (const tag of navigator.languages?.length ? navigator.languages : [navigator.language ?? "en"]) {
    const language = String(tag).toLowerCase();
    if (language.startsWith("pt")) return "pt-BR";
    if (language.startsWith("es")) return "es";
    if (language.startsWith("en")) return "en";
  }
  return "en";
}

export function applyLanguage(code) {
  overwrite(words, english);
  if (catalogs[code]) overwrite(words, catalogs[code]);
  document.documentElement.lang = code;
}
