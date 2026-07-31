const form = document.getElementById("form");
const notice = document.getElementById("notice");
const actionNotice = document.getElementById("action-notice");
const sourceStatus = document.getElementById("source-status");
const set = (name, value) => { form.elements[name].value = value ?? ""; };

async function load() {
  const data = await fetch("/api/status").then((r) => r.json());
  const workbuddy = data.workbuddy || {};
  set("workbuddy_points", workbuddy.points); set("workbuddy_used", workbuddy.used_points); set("workbuddy_reset", workbuddy.reset_text);
  sourceStatus.replaceChildren(...Object.entries(data.collection || {}).map(([name, item]) => {
    const row = document.createElement("div");
    const label = document.createElement("strong"); label.textContent = name.toUpperCase();
    const detail = name === "workbuddy" && workbuddy.balance_error_code
      ? `${workbuddy.balance_error_code} · ${workbuddy.balance_error || "balance read failed"}`
      : `${item.state} · ${item.last_success || "never"}`;
    const state = document.createElement("span"); state.textContent = item.error || detail;
    row.append(label, state); return row;
  }));
}

form.addEventListener("submit", async (event) => {
  event.preventDefault(); const f = form.elements;
  const payload = { workbuddy: { points: Number(f.workbuddy_points.value), used_points: Number(f.workbuddy_used.value), reset_text: f.workbuddy_reset.value } };
  const result = await fetch("/api/status", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(payload) });
  notice.textContent = result.ok ? "已保存。" : "保存失败。";
});
load();

document.getElementById("refresh").addEventListener("click", async () => {
  actionNotice.textContent = "正在刷新…";
  const result = await fetch("/api/refresh", { method: "POST" });
  actionNotice.textContent = result.ok ? "刷新完成。" : "刷新失败。";
  if (result.ok) await load();
});

document.getElementById("reconnect-workbuddy").addEventListener("click", async () => {
  if (!confirm("将重启 WorkBuddy 一次以启用本地读取通道，已打开的会话会自动恢复。继续？")) return;
  actionNotice.textContent = "正在重连 WorkBuddy…";
  const result = await fetch("/api/workbuddy/reconnect", { method: "POST" });
  let payload = {};
  try { payload = await result.json(); } catch { /* keep empty */ }
  if (result.ok) {
    actionNotice.textContent = "已启用本地读取通道。";
    setTimeout(load, 3000);
  } else {
    actionNotice.textContent = `重连失败：${payload.error || payload.error_code || "未知错误"}`;
  }
});
