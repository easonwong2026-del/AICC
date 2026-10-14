const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");

function element() {
  return {
    textContent: "", hidden: false, style: {}, children: [],
    appendChild(child) { this.children.push(child); },
    replaceChildren() { this.children = []; this.textContent = ""; },
    closest() { return this; },
  };
}
const elements = new Map();
const get = (name) => {
  if (!elements.has(name)) elements.set(name, element());
  return elements.get(name);
};
const context = vm.createContext({
  document: { getElementById: get, createElement: element },
  window: { matchMedia: () => ({ matches: false }) },
  location: { search: "" }, navigator: {}, URLSearchParams,
  fetch: () => new Promise(() => {}), setInterval() {}, setTimeout() {},
  localStorage: { getItem: () => null },
});
vm.runInContext(fs.readFileSync("web/app.js", "utf8"), context);
context.render({ codex: { five_hour: { remaining: null }, weekly: { remaining: 0 } } });
assert.equal(get("five-hour-percent").textContent, "--");
assert.equal(get("weekly-percent").textContent, "0%");
context.render({ codex: { source: "Magpie", available: true, stale: true, accounts: [
  { label: "Account 1", weekly: { remaining: 88 } },
  { label: "Account 2", weekly: { remaining: 59 }, five_hour: { remaining: null } },
] } });
assert.equal(get("legacy-quotas").hidden, true);
assert.equal(get("codex-accounts").children.length, 2);
assert.match(get("codex-accounts").children[0].children[0].textContent, /缓存/);
assert.match(get("codex-accounts").children[0].children[1].textContent, /88%/);
assert.match(get("codex-accounts").children[1].children[1].textContent, /59%/);
assert.match(get("codex-accounts").children[1].children[2].textContent, /—/);
assert.equal(get("codex-reset-credits").hidden, true);
context.render({ codex: { accounts: [] } });
assert.equal(get("codex-accounts").textContent, "暂无 Codex 账号");
console.log("Web quota regression passed");
