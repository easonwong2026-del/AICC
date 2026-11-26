package com.aieink.pokedashboard;

import android.app.Activity;
import android.app.Dialog;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.SharedPreferences;
import android.content.res.Configuration;
import android.os.BatteryManager;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.EditText;
import android.widget.Toast;

import java.net.URL;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicBoolean;

public final class MainActivity extends Activity {
    private static final String PREFS = "dashboard";
    static final String KEY_URL = "base_url";
    static final String KEY_HOST_ID = "host_id";
    static final String KEY_CACHE = "last_status";
    static final String KEY_KEEP_AWAKE = "keep_awake";
    static final String KEY_SHOW_BATTERY = "show_battery";
    static final String KEY_LAYOUT_MODE = "layout_mode";
    static final String KEY_SORT_MODE = "sort_mode";
    static final String KEY_PROVIDER_FILTER = "provider_filter";
    private static final String KEY_V11_INITIALIZED = "v11_initialized";
    private static final String TAG = "AICC Discovery";
    /** A bridge answers Empty until its first Magpie read lands; ask again soon. */
    private static final long EMPTY_RETRY_MS = 15_000L;
    private static final int EMPTY_RETRY_LIMIT = 4;

    private final Handler handler = new Handler(Looper.getMainLooper());
    private final ExecutorService network = Executors.newSingleThreadExecutor();
    private final AtomicBoolean fetching = new AtomicBoolean(false);
    private DashboardView dashboard;
    private SharedPreferences preferences;
    private int refreshCount;
    private int emptyRetries;
    private String lastRenderedStatus;
    private boolean resumed;
    private boolean choicePromptShown;
    private volatile boolean destroyed;
    /** Last discovery result, used to show a server's fingerprint when pairing. */
    private volatile List<DiscoveryClient.Candidate> discovered = new ArrayList<>();

    private final Runnable immersiveRetry = this::enterImmersiveMode;
    private final BroadcastReceiver deviceStateReceiver = new BroadcastReceiver() {
        @Override public void onReceive(Context context, Intent intent) {
            String action = intent.getAction();
            if (Intent.ACTION_BATTERY_CHANGED.equals(action)) {
                updateBattery(intent);
                return;
            }
            if (Intent.ACTION_TIME_TICK.equals(action)) {
                if (resumed) dashboard.refreshClock();
                return;
            }
            if (Intent.ACTION_SCREEN_OFF.equals(action)) {
                handler.removeCallbacks(refreshLoop);
                dashboard.settleForSleep();
                return;
            }
            if (Intent.ACTION_SCREEN_ON.equals(action) || Intent.ACTION_USER_PRESENT.equals(action)) {
                scheduleImmersiveMode();
                if (resumed) {
                    dashboard.refreshClock();
                    handler.removeCallbacks(refreshLoop);
                    handler.post(refreshLoop);
                }
            }
        }
    };

    private final Runnable refreshLoop = new Runnable() {
        @Override public void run() {
            fetchStatus();
            handler.postDelayed(this, refreshMinutes() * 60_000L);
        }
    };

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        preferences = getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        initialiseV11Preferences();
        dashboard = new DashboardView(this);
        dashboard.setListener(new DashboardView.Listener() {
            @Override public void onToggleLayoutMode() {
                toggleLayoutMode();
            }

            @Override public void onManualRefresh() {
                Toast.makeText(MainActivity.this, "正在刷新额度…", Toast.LENGTH_SHORT).show();
                handler.removeCallbacks(refreshLoop);
                handler.post(refreshLoop);
            }

            @Override public void onOpenSettings() {
                showSettings();
            }

            @Override public void onStateChanged() {
                lastRenderedStatus = dashboard.getData().renderKey;
            }
        });
        setContentView(dashboard);
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN
                | WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN);
        getWindow().getDecorView().setOnSystemUiVisibilityChangeListener(visibility -> {
            if (resumed) {
                handler.removeCallbacks(immersiveRetry);
                handler.postDelayed(immersiveRetry, 250);
            }
        });
        IntentFilter deviceFilter = new IntentFilter();
        deviceFilter.addAction(Intent.ACTION_BATTERY_CHANGED);
        deviceFilter.addAction(Intent.ACTION_TIME_TICK);
        deviceFilter.addAction(Intent.ACTION_SCREEN_OFF);
        deviceFilter.addAction(Intent.ACTION_SCREEN_ON);
        deviceFilter.addAction(Intent.ACTION_USER_PRESENT);
        if (Build.VERSION.SDK_INT >= 33) {
            registerReceiver(deviceStateReceiver, deviceFilter, Context.RECEIVER_NOT_EXPORTED);
        } else {
            registerReceiver(deviceStateReceiver, deviceFilter);
        }
        applyDisplayMode();
        loadCache();
        updateBattery(registerReceiver(null, new IntentFilter(Intent.ACTION_BATTERY_CHANGED)));
    }

    @Override
    protected void onResume() {
        super.onResume();
        resumed = true;
        scheduleImmersiveMode();
        dashboard.refreshClock();
        handler.removeCallbacks(refreshLoop);
        handler.post(refreshLoop);
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
    }

    @Override
    protected void onPause() {
        resumed = false;
        handler.removeCallbacks(refreshLoop);
        handler.removeCallbacks(immersiveRetry);
        dashboard.settleForSleep();
        super.onPause();
    }

    @Override
    protected void onDestroy() {
        destroyed = true;
        handler.removeCallbacksAndMessages(null);
        dashboard.settleForSleep();
        try { unregisterReceiver(deviceStateReceiver); } catch (IllegalArgumentException ignored) { }
        network.shutdownNow();
        super.onDestroy();
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) scheduleImmersiveMode();
    }

    @Override
    public void onConfigurationChanged(Configuration configuration) {
        super.onConfigurationChanged(configuration);
        dashboard.invalidate();
        scheduleImmersiveMode();
    }

    private void toggleLayoutMode() {
        String current = preferences.getString(KEY_LAYOUT_MODE, DashboardData.LAYOUT_GRID);
        String next = DashboardData.LAYOUT_CLOCK.equals(current)
                ? DashboardData.LAYOUT_GRID
                : DashboardData.LAYOUT_CLOCK;
        preferences.edit().putString(KEY_LAYOUT_MODE, next).apply();
        reapplyFromCache(false);
        Toast.makeText(this,
                DashboardData.LAYOUT_CLOCK.equals(next) ? "已切换：桌搭时钟" : "已切换：瑞士网格",
                Toast.LENGTH_SHORT).show();
    }

    private void reapplyFromCache(boolean preserveNavigation) {
        DashboardData current = dashboard.getData();
        String raw = preferences.getString(KEY_CACHE, null);
        if (raw == null) {
            current.layoutMode = preferences.getString(KEY_LAYOUT_MODE, DashboardData.LAYOUT_GRID);
            current.sortMode = preferences.getString(KEY_SORT_MODE, DashboardData.SORT_LOWEST);
            current.providerFilter = preferences.getString(KEY_PROVIDER_FILTER, "");
            if (!preserveNavigation) {
                current.page = 0;
                current.selectedAccountId = null;
            }
            current.clampPage();
            current.refreshRenderKey();
            lastRenderedStatus = current.renderKey;
            dashboard.setData(current);
            return;
        }
        try {
            DashboardData next = parseData(raw);
            next.offline = current.offline;
            if (preserveNavigation) {
                next.page = current.page;
                next.selectedAccountId = current.selectedAccountId;
                next.clampPage();
                next.refreshRenderKey();
            }
            lastRenderedStatus = next.renderKey;
            dashboard.setData(next);
        } catch (Exception ignored) { }
    }

    private void fetchStatus() {
        if (!fetching.compareAndSet(false, true)) return;
        network.execute(() -> {
            DiscoveryClient.Candidate selected = null;
            List<DiscoveryClient.Candidate> found = new ArrayList<>();
            try {
                String saved = normaliseBaseUrl(preferences.getString(KEY_URL, null));
                String preferredHostId = preferences.getString(KEY_HOST_ID, null);
                String token = BridgeCredential.load(preferences);
                BridgeTls.Identity identity = BridgeCredential.identity(preferences);
                selected = saved == null ? null
                        : DiscoveryClient.probe(new DiscoveryClient.Candidate(saved), identity, token, preferredHostId);
                if (selected != null && preferredHostId != null
                        && !preferredHostId.equals(selected.hostId)) {
                    Log.w(TAG, "Saved address belongs to a different server: " + saved);
                    selected = null;
                }
                if (selected == null) {
                    found = DiscoveryClient.findServers();
                    discovered = found;
                    selected = DiscoveryClient.choose(found, preferredHostId);
                    if (selected != null && selected.pairingRequired && token != null && identity != null) {
                        selected = DiscoveryClient.probe(selected, identity, token, preferredHostId);
                    }
                }
            } catch (RuntimeException error) {
                // Never leave the in-flight flag set: one unexpected error here
                // would otherwise stop every later refresh until a restart.
                Log.w(TAG, "Status refresh failed", error);
                selected = null;
            } finally {
                fetching.set(false);
            }
            final DiscoveryClient.Candidate result = selected;
            final List<DiscoveryClient.Candidate> alternatives = found;
            if (destroyed) return;
            handler.post(() -> {
                if (destroyed) return;
                if (result != null) {
                    selectServer(result, null);
                } else {
                    if (resumed) dashboard.setOffline(true);
                    if (resumed && alternatives.size() > 1 && !choicePromptShown) {
                        choicePromptShown = true;
                        showServerChoices(alternatives, null, null);
                    }
                }
            });
        });
    }

    private void selectServer(DiscoveryClient.Candidate candidate, EditText urlField) {
        if (candidate.pairingRequired) {
            if (urlField != null) urlField.setText(candidate.url);
            else preferences.edit().putString(KEY_URL, candidate.url).apply();
            dashboard.setOffline(true);
            if (resumed && !choicePromptShown && urlField == null) {
                choicePromptShown = true;
                Toast.makeText(this, "请在设置中输入 Mac AICC 的配对码", Toast.LENGTH_LONG).show();
                showSettings();
            }
            return;
        }
        String previous = preferences.getString(KEY_URL, null);
        String hostId = candidate.hostId == null || candidate.hostId.isEmpty() ? null : candidate.hostId;
        if (!candidate.url.equals(previous)
                || !java.util.Objects.equals(hostId, preferences.getString(KEY_HOST_ID, null))) {
            SharedPreferences.Editor editor = preferences.edit().putString(KEY_URL, candidate.url);
            if (hostId == null) editor.remove(KEY_HOST_ID);
            else editor.putString(KEY_HOST_ID, hostId);
            editor.apply();
        }
        if (previous != null && !previous.equals(candidate.url)) {
            Log.i(TAG, "Saved base URL migrated: " + previous + " -> " + candidate.url);
        }
        Log.i(TAG, "Candidate selected: " + candidate.url);
        if (urlField != null) urlField.setText(candidate.url);
        if (candidate.statusBody == null) {
            dashboard.setOffline(true);
            return;
        }
        if (!java.util.Objects.equals(candidate.statusBody, preferences.getString(KEY_CACHE, null))) {
            preferences.edit().putString(KEY_CACHE, candidate.statusBody).apply();
        }
        if (resumed) {
            try {
                candidate.data = parseData(candidate.statusBody);
            } catch (Exception ignored) {
                return;
            }
            DashboardData current = dashboard.getData();
            candidate.data.page = current.page;
            candidate.data.selectedAccountId = current.selectedAccountId;
            candidate.data.clampPage();
            candidate.data.refreshRenderKey();

            if (!candidate.data.renderKey.equals(lastRenderedStatus)) {
                lastRenderedStatus = candidate.data.renderKey;
                dashboard.setData(candidate.data);
                refreshCount++;
                if (refreshCount % 12 == 0) dashboard.flashRefresh();
            } else {
                dashboard.updateSyncTime(candidate.data.updatedAt);
                dashboard.setOffline(false);
            }
            retryWhenEmpty(candidate.data.available);
        }
        choicePromptShown = false;
    }

    /**
     * The bridge can only answer with the accounts it has read so far, which is
     * "none" for the first seconds after the Mac app starts (and for as long as
     * Magpie is not up).  Poll again a few times before falling back.
     */
    private void retryWhenEmpty(boolean available) {
        if (available) { emptyRetries = 0; return; }
        if (emptyRetries >= EMPTY_RETRY_LIMIT) return;
        emptyRetries++;
        handler.removeCallbacks(refreshLoop);
        handler.postDelayed(refreshLoop, EMPTY_RETRY_MS);
    }

    private void forceDiscovery(EditText urlField, Runnable onDiscovered) {
        if (!fetching.compareAndSet(false, true)) {
            Toast.makeText(this, "正在同步，请稍后重试", Toast.LENGTH_SHORT).show();
            return;
        }
        network.execute(() -> {
            List<DiscoveryClient.Candidate> found = DiscoveryClient.findServers();
            discovered = found;
            fetching.set(false);
            if (destroyed) return;
            handler.post(() -> {
                if (destroyed) return;
                if (found.isEmpty()) {
                    Toast.makeText(this,
                            "未找到 AICC。请在 Mac 的「设置 → 墨水屏连接」打开「允许局域网设备连接」，并确认在同一网络",
                            Toast.LENGTH_LONG).show();
                } else if (found.size() == 1) {
                    selectServer(found.get(0), urlField);
                    if (onDiscovered != null) onDiscovered.run();
                } else {
                    showServerChoices(found, urlField, onDiscovered);
                }
            });
        });
    }

    private void showServerChoices(List<DiscoveryClient.Candidate> servers, EditText urlField, Runnable onSelected) {
        String[] labels = new String[servers.size()];
        for (int index = 0; index < servers.size(); index++) {
            DiscoveryClient.Candidate server = servers.get(index);
            String name = server.hostname == null || server.hostname.isEmpty() ? server.name : server.hostname;
            labels[index] = name + (server.platform == null || server.platform.isEmpty() ? "" : " · " + server.platform)
                    + "\n" + server.url + (server.stale ? " · 部分额度为缓存" : "");
        }
        new android.app.AlertDialog.Builder(this)
                .setTitle("选择 AICC 服务器")
                .setItems(labels, (dialog, index) -> {
                    selectServer(servers.get(index), urlField);
                    if (onSelected != null) onSelected.run();
                })
                .setNegativeButton("取消", null)
                .show();
    }

    private void loadCache() {
        String raw = preferences.getString(KEY_CACHE, null);
        if (raw == null) {
            DashboardData initial = dashboard.getData();
            initial.layoutMode = preferences.getString(KEY_LAYOUT_MODE, DashboardData.LAYOUT_GRID);
            initial.sortMode = preferences.getString(KEY_SORT_MODE, DashboardData.SORT_LOWEST);
            initial.providerFilter = preferences.getString(KEY_PROVIDER_FILTER, "");
            dashboard.setData(initial);
            return;
        }
        try {
            DashboardData data = parseData(raw);
            data.offline = true;
            lastRenderedStatus = data.renderKey;
            dashboard.setData(data);
        } catch (Exception ignored) { }
    }

    /** Fingerprint of the server at this address, from discovery or the stored pin. */
    private String pinFor(String base) {
        for (DiscoveryClient.Candidate candidate : discovered) {
            if (candidate.url.equals(base) && candidate.pin != null && candidate.pin.length() == 44) {
                return candidate.pin;
            }
        }
        String stored = BridgeCredential.pin(preferences);
        String storedUrl = normaliseBaseUrl(preferences.getString(KEY_URL, null));
        if (stored != null && base.equals(storedUrl)) return stored;
        return null;
    }

    /** Grouped in fours, so a human can compare it with the Mac character by character. */
    private static String formatPin(String pin) {
        StringBuilder grouped = new StringBuilder();
        for (int index = 0; index < pin.length(); index += 4) {
            if (grouped.length() > 0) grouped.append(' ');
            grouped.append(pin, index, Math.min(pin.length(), index + 4));
        }
        return grouped.toString();
    }

    Dialog showSettings() {
        return SettingsDialog.show(this, new SettingsDialog.Host() {
            @Override public SharedPreferences prefs() {
                return preferences;
            }

            @Override public String currentPinFor(String baseUrl) {
                return pinFor(baseUrl);
            }

            @Override public boolean isPaired() {
                return BridgeCredential.load(preferences) != null
                        && BridgeCredential.identity(preferences) != null;
            }

            @Override public List<DashboardData.ProviderOption> availableProviders() {
                return dashboard.getData().availableProviders();
            }

            @Override public int totalAccountCount() {
                return dashboard.getData().accounts.size();
            }

            @Override public void onDiscoverRequested(EditText urlField, Runnable onDiscovered) {
                forceDiscovery(urlField, onDiscovered);
            }

            @Override public void onPairRequested(String rawUrl, String pairingCode, Button pairButton,
                                                  EditText codeField, Dialog dialog) {
                String base = normaliseBaseUrl(rawUrl);
                if (base == null || !pairingCode.matches("[0-9]{8}")) {
                    Toast.makeText(MainActivity.this, "请输入电脑地址和 8 位配对码", Toast.LENGTH_SHORT).show();
                    return;
                }
                String pin = pinFor(base);
                if (pin == null) {
                    Toast.makeText(MainActivity.this, "请先点「自动发现」以读出该电脑的证书指纹",
                            Toast.LENGTH_LONG).show();
                    return;
                }
                new android.app.AlertDialog.Builder(MainActivity.this)
                        .setTitle("核对指纹")
                        .setMessage("请确认下面这段与 Mac 上 AICC 设置页显示的指纹完全一致：\n\n"
                                + formatPin(pin)
                                + "\n\n不一致说明这台机器不是你的 Mac，请取消。")
                        .setNegativeButton("取消", null)
                        .setPositiveButton("一致，开始配对", (confirmed, which) -> {
                            pairButton.setEnabled(false);
                            network.execute(() -> {
                                boolean success = false;
                                String failure = "配对失败，请生成新配对码后重试";
                                try {
                                    org.json.JSONObject result = DiscoveryClient.pair(base, pairingCode, pin);
                                    String token = result.getString("token");
                                    String hostId = result.getString("host_id");
                                    String caPem = result.optString("ca_pem", "");
                                    String returnedPin = result.optString("spki", pin);
                                    if (token.length() < 32 || hostId.isEmpty()
                                            || !caPem.contains("BEGIN CERTIFICATE")) {
                                        throw new IllegalStateException("Incomplete pairing response");
                                    }
                                    if (!returnedPin.equals(pin)) {
                                        throw new IllegalStateException("Server identity changed during pairing");
                                    }
                                    BridgeCredential.save(preferences, token, hostId, pin, caPem);
                                    preferences.edit().putString(KEY_HOST_ID, hostId).putString(KEY_URL, base)
                                            .remove(KEY_CACHE).commit();
                                    success = true;
                                } catch (Exception error) {
                                    Log.w(TAG, "Pairing failed: " + error.getClass().getSimpleName());
                                }
                                boolean paired = success;
                                handler.post(() -> {
                                    if (destroyed) return;
                                    pairButton.setEnabled(true);
                                    codeField.setText("");
                                    Toast.makeText(MainActivity.this, paired ? "配对成功" : failure,
                                            Toast.LENGTH_LONG).show();
                                    if (paired) {
                                        dialog.dismiss();
                                        fetchStatus();
                                    }
                                });
                            });
                        })
                        .show();
            }

            @Override public void onSaveSettings(String rawUrl, String layoutMode, String sortMode,
                                                 String providerFilter, String accountMode,
                                                 int intervalMinutes, boolean keepAwake,
                                                 boolean showBattery, boolean autoStart, Dialog dialog) {
                String base = normaliseBaseUrl(rawUrl);
                if (base != null) {
                    try {
                        URL parsed = new URL(base);
                        if ((!"http".equals(parsed.getProtocol()) && !"https".equals(parsed.getProtocol()))
                                || parsed.getHost().isEmpty()) {
                            throw new IllegalArgumentException();
                        }
                    } catch (Exception error) {
                        Toast.makeText(MainActivity.this, "服务器地址无效", Toast.LENGTH_SHORT).show();
                        return;
                    }
                }
                SharedPreferences.Editor editor = preferences.edit()
                        .putString(KEY_LAYOUT_MODE, layoutMode)
                        .putString(KEY_SORT_MODE, sortMode)
                        .putString(KEY_PROVIDER_FILTER, providerFilter == null ? "" : providerFilter)
                        .putString("account_mode", accountMode)
                        .putInt("refresh_minutes", intervalMinutes)
                        .putBoolean(KEY_KEEP_AWAKE, keepAwake)
                        .putBoolean(KEY_SHOW_BATTERY, showBattery)
                        .putBoolean("auto_start", autoStart);
                if (base == null) {
                    editor.remove(KEY_URL).remove(KEY_HOST_ID);
                } else {
                    if (!base.equals(preferences.getString(KEY_URL, null))) editor.remove(KEY_HOST_ID);
                    editor.putString(KEY_URL, base);
                }
                editor.apply();
                applyDisplayMode();
                updateBattery(registerReceiver(null, new IntentFilter(Intent.ACTION_BATTERY_CHANGED)));
                reapplyFromCache(false);
                handler.removeCallbacks(refreshLoop);
                handler.post(refreshLoop);
                dialog.dismiss();
            }

            @Override public void onImmediateRefresh() {
                handler.removeCallbacks(refreshLoop);
                handler.post(refreshLoop);
            }
        });
    }

    private DashboardData parseData(String raw) throws Exception {
        return DashboardData.parse(
                raw,
                preferences.getString("account_mode", "all"),
                preferences.getString(KEY_LAYOUT_MODE, DashboardData.LAYOUT_GRID),
                preferences.getString(KEY_SORT_MODE, DashboardData.SORT_LOWEST),
                preferences.getString(KEY_PROVIDER_FILTER, "")
        );
    }

    private int refreshMinutes() {
        return Math.max(1, Math.min(60, preferences.getInt("refresh_minutes", 5)));
    }

    private String normaliseBaseUrl(String value) {
        return DiscoveryClient.normaliseBaseUrl(value);
    }

    private void applyDisplayMode() {
        if (preferences.getBoolean(KEY_KEEP_AWAKE, false)) {
            getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        } else {
            getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        }
        scheduleImmersiveMode();
    }

    private void initialiseV11Preferences() {
        if (preferences.getBoolean(KEY_V11_INITIALIZED, false)) return;
        SharedPreferences.Editor editor = preferences.edit().putBoolean(KEY_V11_INITIALIZED, true);
        if (!preferences.contains(KEY_KEEP_AWAKE)) editor.putBoolean(KEY_KEEP_AWAKE, false);
        if (!preferences.contains(KEY_SHOW_BATTERY)) editor.putBoolean(KEY_SHOW_BATTERY, true);
        editor.apply();
    }

    private void updateBattery(Intent battery) {
        if (dashboard == null) return;
        int percent = -1;
        boolean charging = false;
        if (battery != null) {
            int level = battery.getIntExtra(BatteryManager.EXTRA_LEVEL, -1);
            int scale = battery.getIntExtra(BatteryManager.EXTRA_SCALE, 100);
            if (level >= 0 && scale > 0) percent = Math.round(level * 100f / scale);
            int status = battery.getIntExtra(BatteryManager.EXTRA_STATUS, -1);
            charging = status == BatteryManager.BATTERY_STATUS_CHARGING
                    || status == BatteryManager.BATTERY_STATUS_FULL;
        }
        dashboard.setDeviceBattery(percent, charging,
                preferences.getBoolean(KEY_SHOW_BATTERY, true));
    }

    private void scheduleImmersiveMode() {
        enterImmersiveMode();
        handler.removeCallbacks(immersiveRetry);
        handler.postDelayed(immersiveRetry, 300);
        handler.postDelayed(immersiveRetry, 1200);
    }

    @SuppressWarnings("deprecation")
    private void enterImmersiveMode() {
        if (Build.VERSION.SDK_INT >= 30) {
            getWindow().setDecorFitsSystemWindows(false);
            WindowInsetsController controller = getWindow().getInsetsController();
            if (controller != null) {
                controller.hide(WindowInsets.Type.systemBars());
                controller.setSystemBarsBehavior(WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
            }
        } else {
            getWindow().getDecorView().setSystemUiVisibility(
                    View.SYSTEM_UI_FLAG_FULLSCREEN
                            | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                            | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                            | View.SYSTEM_UI_FLAG_LOW_PROFILE
                            | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                            | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                            | View.SYSTEM_UI_FLAG_LAYOUT_STABLE);
        }
    }
}
