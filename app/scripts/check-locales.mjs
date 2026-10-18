// Every language must have the same keys as the English one, and each one must be a real word list:
// a missing translation shows English, but a key that is not in English is dead.
import { words } from "../src/vocabulary.js";
import ptBR from "../src/locales/pt-BR.js";
import es from "../src/locales/es.js";

const keys = (value, prefix = "") =>
  Object.entries(value).flatMap(([key, inner]) =>
    inner && typeof inner === "object" && !Array.isArray(inner) ? keys(inner, `${prefix}${key}.`) : [`${prefix}${key}`],
  );

const english = new Set(keys(words));
const skipped = new Set(["brand"]);
let failed = false;
for (const [name, catalog] of [["pt-BR", ptBR], ["es", es]]) {
  const have = new Set(keys(catalog));
  const missing = [...english].filter((key) => !have.has(key) && !skipped.has(key));
  const extra = [...have].filter((key) => !english.has(key));
  if (missing.length || extra.length) {
    failed = true;
    console.error(`${name}: missing ${JSON.stringify(missing)} extra ${JSON.stringify(extra)}`);
  }
}
if (failed) process.exit(1);
console.log("the three languages have the same words");
