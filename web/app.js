const $ = (id) => document.getElementById(id);
const quota = (value) => Math.max(0, Math.min(100, Number(value) || 0));
let retryTimer = null;

function showCollector(name, metadata) {
  const target = $(`${name}-collector`);
  if (!target) return;
  const item = metadata?.[name] || {};
  const age = Number.isFinite(Number(item.age_seconds)) ? `${item.age_seconds}s 前` : "尚无成功记录";
  if (item.state === "refreshing") target.textContent = `正在刷新 · 上次 ${age}`;
  else if (item.state === "error") target.textContent = `采集失败 · ${item.error || age}`;
  else target.textContent = item.last_success ? `最后成功 ${item.last_success} · ${age}` : "等待首次采集";
}

function showQuota(prefix, data) {
  const valid = Number.isFinite(Number(data.remaining));
  const remaining = valid ? quota(data.remaining) : 0;
  $(`${prefix}-percent`).textContent = valid ? `${remaining}%` : "--";
  $(`${prefix}-bar`).style.width = `${remaining}%`;
  $(`${prefix}-reset`).textContent = `重置 ${data.reset || "--"}`;
}

function render(data) {
  const codex = data.codex || {};
  const deepseek = data.deepseek || {};
  const workbuddy = data.workbuddy || {};
  const system = data.system || {};
  const collection = data.collection || {};
  showQuota("five-hour", codex.five_hour || {});
  showQuota("weekly", codex.weekly || {});
  $("codex-source").textContent = codex.source || "--";
  const resetCredits = codex.reset_credits || {};
  $("codex-reset-credits").textContent = resetCredits.provided
    ? `${resetCredits.available_count} 次`
    : "未提供";
  $("codex-reset-expiry").textContent = resetCredits.provided
    ? (resetCredits.next_expiry ? `最近到期 ${resetCredits.next_expiry}` : "官方账户数据")
    : "当前账户未返回该项";
  const buckets = Array.isArray(codex.limit_buckets) ? codex.limit_buckets : [];
  $("codex-buckets").textContent = buckets.length > 1
    ? `${buckets.length} 组额度 · ${buckets.map((item) => item.name).join(" / ")}`
    : "";
  const age = Number.isFinite(Number(codex.age_seconds)) ? ` · ${codex.age_seconds}s ago` : "";
  $("codex-state").textContent = codex.available
    ? `${codex.stale ? "Cached" : "Connected"}${age} · refreshes every ${codex.refresh_seconds || 60}s`
    : `Not connected · ${codex.state || "--"}`;
  const workBuddyPoints = Number(workbuddy.points);
  $("workbuddy-points").textContent = Number.isFinite(workBuddyPoints)
    ? workBuddyPoints.toLocaleString()
    : "--";
  const used = workbuddy.auto_used_credits ?? workbuddy.used_points ?? 0;
  const balanceAge = Number(workbuddy.balance_age_seconds);
  const balanceAgeText = Number.isFinite(balanceAge) ? ` · ${balanceAge}s 前` : "";
  const balanceError = workbuddy.balance_error_code ? ` · ${workbuddy.balance_error_code}` : "";
  $("workbuddy-state").textContent = `${workbuddy.balance_state || "Unavailable"}${balanceAgeText}${balanceError}`;
  $("workbuddy-used").textContent = Number(used).toLocaleString();
  $("workbuddy-reset").textContent = workbuddy.reset_text || "--";
  $("workbuddy-source").textContent = workbuddy.usage_source ? `Source: ${workbuddy.usage_source}` : "";
  const balance = (deepseek.balances || [])[0];
  $("deepseek-state").textContent = deepseek.status || "--";
  const usage = (deepseek.usage || [])[0];
  $("deepseek-balance").textContent = balance ? balance.total_balance : "--";
  $("deepseek-balance-currency").textContent = balance?.currency || "";
  $("deepseek-usage").textContent = usage ? usage.used_today : "--";
  $("deepseek-usage-currency").textContent = usage?.currency || balance?.currency || "";
  $("deepseek-detail").textContent = deepseek.source || "--";
  $("system-state").textContent = system.status || "--";
  $("system-label").textContent = system.label || "--";
  $("system-metrics").textContent = `CPU ${system.cpu || "--"} · RAM ${system.ram || "--"}`;
  $("gpu-metrics").textContent = system.gpu || "";
  $("updated-at").textContent = data.updated_at || "--";
  $("connection").textContent = "●";
  ["codex", "workbuddy", "deepseek", "system"].forEach((name) => showCollector(name, collection));
  if (Object.values(collection).some((item) => item?.state === "refreshing") && !retryTimer) {
    retryTimer = setTimeout(() => { retryTimer = null; refresh(); }, 3500);
  }
}

async function refresh() {
  try {
    const response = await fetch("/api/status", { cache: "no-store" });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    const data = await response.json();
    render(data);
    localStorage.setItem("ai-eink:last-status", JSON.stringify(data));
  } catch {
    const cached = localStorage.getItem("ai-eink:last-status");
    if (cached) {
      try { render(JSON.parse(cached)); } catch { /* ignore invalid cache */ }
    }
    $("connection").textContent = "○";
  }
}

refresh();
setInterval(refresh, 300000);

if ("serviceWorker" in navigator) navigator.serviceWorker.register("/service-worker.js").catch(() => {});

const kiosk = new URLSearchParams(location.search).get("kiosk") === "1"
  || window.matchMedia("(display-mode: standalone)").matches
  || window.matchMedia("(display-mode: fullscreen)").matches;
let wakeLock = null;

async function keepAwake() {
  if (!("wakeLock" in navigator)) return;
  try { wakeLock = await navigator.wakeLock.request("screen"); } catch { wakeLock = null; }
}

async function enterKiosk() {
  try {
    if (!document.fullscreenElement && document.documentElement.requestFullscreen) {
      await document.documentElement.requestFullscreen();
    }
  } catch { /* PWA standalone mode is already fullscreen */ }
  await keepAwake();
  $("kiosk-gate").hidden = true;
  localStorage.setItem("ai-eink:kiosk-started", "1");
}

if (kiosk) {
  const standalone = window.matchMedia("(display-mode: standalone)").matches
    || window.matchMedia("(display-mode: fullscreen)").matches;
  if (standalone) keepAwake();
  else {
    $("kiosk-gate").hidden = false;
    $("kiosk-start").addEventListener("click", enterKiosk);
  }
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible" && (!wakeLock || wakeLock.released)) keepAwake();
  });
  setInterval(() => {
    document.documentElement.classList.add("eink-refresh");
    setTimeout(() => document.documentElement.classList.remove("eink-refresh"), 900);
    setTimeout(() => location.reload(), 1500);
  }, 60 * 60 * 1000);
}
