package com.aieink.pokedashboard;

import android.app.Activity;
import android.app.Dialog;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.SharedPreferences;
import android.content.res.Configuration;
import android.content.res.ColorStateList;
import android.graphics.Color;
import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.GradientDrawable;
import android.os.BatteryManager;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.util.Log;
import android.text.InputType;
import android.view.View;
import android.view.Gravity;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.RadioButton;
import android.widget.RadioGroup;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import java.net.URL;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicBoolean;

public final class MainActivity extends Activity {
    private static final String PREFS = "dashboard";
    private static final String KEY_URL = "base_url";
    private static final String KEY_HOST_ID = "host_id";
    private static final String KEY_CACHE = "last_status";
    private static final String KEY_KEEP_AWAKE = "keep_awake";
    private static final String KEY_SHOW_BATTERY = "show_battery";
    private static final String KEY_V11_INITIALIZED = "v11_initialized";
    private static final String TAG = "AICC Discovery";

    private final Handler handler = new Handler(Looper.getMainLooper());
    private final ExecutorService network = Executors.newSingleThreadExecutor();
    private final AtomicBoolean fetching = new AtomicBoolean(false);
    private DashboardView dashboard;
    private SharedPreferences preferences;
    private int refreshCount;
    private boolean resumed;
    private boolean choicePromptShown;
    private volatile boolean destroyed;

    private final Runnable immersiveRetry = this::enterImmersiveMode;
    private final BroadcastReceiver deviceStateReceiver = new BroadcastReceiver() {
        @Override public void onReceive(Context context, Intent intent) {
            String action = intent.getAction();
            if (Intent.ACTION_BATTERY_CHANGED.equals(action)) {
                updateBattery(intent);
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
        dashboard.setOnLongClickListener(view -> {
            showSettings();
            return true;
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

    private void fetchStatus() {
        if (!fetching.compareAndSet(false, true)) return;
        network.execute(() -> {
            String saved = normaliseBaseUrl(preferences.getString(KEY_URL, null));
            String preferredHostId = preferences.getString(KEY_HOST_ID, null);
            DiscoveryClient.Candidate selected = saved == null ? null
                    : DiscoveryClient.probe(new DiscoveryClient.Candidate(saved));
            if (selected != null && preferredHostId != null
                    && !preferredHostId.equals(selected.hostId)) {
                Log.w(TAG, "Saved address belongs to a different server: " + saved);
                selected = null;
            }
            List<DiscoveryClient.Candidate> found = new ArrayList<>();
            if (selected == null) {
                found = DiscoveryClient.findServers();
                selected = DiscoveryClient.choose(found, preferredHostId);
            }
            final DiscoveryClient.Candidate result = selected;
            final List<DiscoveryClient.Candidate> alternatives = found;
            fetching.set(false);
            if (destroyed) return;
            handler.post(() -> {
                if (destroyed) return;
                if (result != null) {
                    selectServer(result, null);
                } else {
                    if (resumed) dashboard.setOffline(true);
                    if (resumed && alternatives.size() > 1 && !choicePromptShown) {
                        choicePromptShown = true;
                        showServerChoices(alternatives, null);
                    }
                }
            });
        });
    }

    private void selectServer(DiscoveryClient.Candidate candidate, EditText urlField) {
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
        if (!candidate.statusBody.equals(preferences.getString(KEY_CACHE, null))) {
            preferences.edit().putString(KEY_CACHE, candidate.statusBody).apply();
        }
        if (resumed) {
            dashboard.setData(candidate.data);
            refreshCount++;
            if (refreshCount % 12 == 0) dashboard.flashRefresh();
        }
        choicePromptShown = false;
    }

    private void forceDiscovery(EditText urlField) {
        if (!fetching.compareAndSet(false, true)) {
            Toast.makeText(this, "正在同步，请稍后重试", Toast.LENGTH_SHORT).show();
            return;
        }
        network.execute(() -> {
            List<DiscoveryClient.Candidate> found = DiscoveryClient.findServers();
            fetching.set(false);
            if (destroyed) return;
            handler.post(() -> {
                if (destroyed) return;
                if (found.isEmpty()) {
                    Toast.makeText(this, "未找到可用的 AICC 服务器", Toast.LENGTH_SHORT).show();
                } else if (found.size() == 1) {
                    selectServer(found.get(0), urlField);
                } else {
                    showServerChoices(found, urlField);
                }
            });
        });
    }

    private void showServerChoices(List<DiscoveryClient.Candidate> servers, EditText urlField) {
        String[] labels = new String[servers.size()];
        for (int index = 0; index < servers.size(); index++) {
            DiscoveryClient.Candidate server = servers.get(index);
            String name = server.hostname == null || server.hostname.isEmpty() ? server.name : server.hostname;
            labels[index] = name + (server.platform == null || server.platform.isEmpty() ? "" : " · " + server.platform)
                    + "\n" + server.url + (server.stale ? " · 部分额度为缓存" : "");
        }
        new android.app.AlertDialog.Builder(this)
                .setTitle("选择 AICC 服务器")
                .setItems(labels, (dialog, index) -> selectServer(servers.get(index), urlField))
                .setNegativeButton("取消", null)
                .show();
    }

    private void loadCache() {
        String raw = preferences.getString(KEY_CACHE, null);
        if (raw == null) return;
        try {
            DashboardData data = DashboardData.parse(raw);
            data.offline = true;
            dashboard.setData(data);
        } catch (Exception ignored) { }
    }

    private void showSettings() {
        final int padding = dp(18);
        final int ink = Color.rgb(17, 17, 17);
        final int muted = Color.rgb(85, 85, 85);
        Dialog dialog = new Dialog(this);
        dialog.requestWindowFeature(android.view.Window.FEATURE_NO_TITLE);
        dialog.setCanceledOnTouchOutside(true);
        LinearLayout panel = new LinearLayout(this);
        panel.setOrientation(LinearLayout.VERTICAL);
        panel.setPadding(padding, dp(14), padding, 0);
        panel.setBackground(outline(Color.WHITE, ink, dp(2), dp(14)));

        TextView title = settingText("AI E-Ink 设置", 24, ink, true);
        title.setGravity(Gravity.CENTER);
        panel.addView(title, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, dp(48)));
        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(dp(4), 0, dp(4), 0);

        TextView hint = settingText("电脑地址（可手工输入）", 16, muted, false);
        EditText url = new EditText(this);
        styleInput(url, 17, ink);
        url.setSingleLine(true);
        url.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI);
        url.setText(preferences.getString(KEY_URL, ""));
        Button chooseServer = settingButton("选择服务器");
        chooseServer.setOnClickListener(view -> forceDiscovery(url));
        LinearLayout addressHeader = new LinearLayout(this);
        addressHeader.setOrientation(LinearLayout.HORIZONTAL);
        addressHeader.addView(hint, new LinearLayout.LayoutParams(0, dp(38), 1f));
        addressHeader.addView(chooseServer, new LinearLayout.LayoutParams(dp(120), dp(38)));

        TextView intervalHint = settingText("刷新间隔（分钟）", 16, muted, false);
        EditText interval = new EditText(this);
        styleInput(interval, 17, ink);
        interval.setSingleLine(true);
        interval.setInputType(InputType.TYPE_CLASS_NUMBER);
        interval.setText(String.valueOf(refreshMinutes()));

        TextView modeHint = settingText("显示模式（省电模式需在 BOOX 中启用透明屏保）", 14, muted, false);
        RadioGroup displayMode = new RadioGroup(this);
        displayMode.setOrientation(RadioGroup.VERTICAL);
        RadioButton screenSaverMode = new RadioButton(this);
        styleChoice(screenSaverMode, "省电屏保：允许休眠，唤醒后立即更新\n（推荐）", 17, true);
        RadioButton alwaysOnMode = new RadioButton(this);
        styleChoice(alwaysOnMode, "桌面常亮：保持屏幕点亮并定时更新", 17, false);
        displayMode.addView(screenSaverMode);
        displayMode.addView(alwaysOnMode);
        boolean keepAwake = preferences.getBoolean(KEY_KEEP_AWAKE, false);
        screenSaverMode.setChecked(!keepAwake);
        alwaysOnMode.setChecked(keepAwake);

        android.widget.CheckBox showBattery = new android.widget.CheckBox(this);
        styleChoice(showBattery, "在面板右上角显示 Poke4S 电量", 17, false);
        showBattery.setChecked(preferences.getBoolean(KEY_SHOW_BATTERY, true));
        android.widget.CheckBox autoStart = new android.widget.CheckBox(this);
        styleChoice(autoStart, "开机后尝试自动启动", 17, false);
        autoStart.setChecked(preferences.getBoolean("auto_start", true));

        content.addView(addressHeader);
        content.addView(url, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, dp(42)));
        content.addView(settingDivider());
        content.addView(intervalHint);
        content.addView(interval, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, dp(42)));
        content.addView(settingDivider());
        content.addView(modeHint);
        content.addView(displayMode);
        content.addView(showBattery);
        content.addView(autoStart);

        ScrollView scroll = new ScrollView(this);
        scroll.setFillViewport(true);
        scroll.addView(content);
        panel.addView(scroll, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f));
        panel.addView(settingDivider());
        LinearLayout actions = new LinearLayout(this);
        actions.setOrientation(LinearLayout.HORIZONTAL);
        Button cancel = settingButton("取消");
        cancel.setOnClickListener(view -> dialog.dismiss());
        Button discover = settingButton("自动发现");
        discover.setOnClickListener(view -> forceDiscovery(url));
        Button save = settingButton("保存");
        save.setOnClickListener(view -> {
                    String base = normaliseBaseUrl(url.getText().toString());
                    if (base != null) {
                        try {
                            URL parsed = new URL(base);
                            if ((!"http".equals(parsed.getProtocol()) && !"https".equals(parsed.getProtocol()))
                                    || parsed.getHost().isEmpty()) throw new IllegalArgumentException();
                        } catch (Exception error) {
                            Toast.makeText(this, "服务器地址无效", Toast.LENGTH_SHORT).show();
                            return;
                        }
                    }
                    int minutes = 5;
                    try { minutes = Integer.parseInt(interval.getText().toString()); }
                    catch (NumberFormatException ignored) { }
                    minutes = Math.max(1, Math.min(60, minutes));
                    SharedPreferences.Editor editor = preferences.edit()
                            .putInt("refresh_minutes", minutes)
                            .putBoolean(KEY_KEEP_AWAKE, alwaysOnMode.isChecked())
                            .putBoolean(KEY_SHOW_BATTERY, showBattery.isChecked())
                            .putBoolean("auto_start", autoStart.isChecked());
                    if (base == null) editor.remove(KEY_URL).remove(KEY_HOST_ID);
                    else {
                        if (!base.equals(preferences.getString(KEY_URL, null))) editor.remove(KEY_HOST_ID);
                        editor.putString(KEY_URL, base);
                    }
                    editor.apply();
                    applyDisplayMode();
                    updateBattery(registerReceiver(null,
                            new IntentFilter(Intent.ACTION_BATTERY_CHANGED)));
                    handler.removeCallbacks(refreshLoop);
                    handler.post(refreshLoop);
                    dialog.dismiss();
        });
        actions.addView(cancel, actionParams());
        actions.addView(actionDivider());
        actions.addView(discover, actionParams());
        actions.addView(actionDivider());
        actions.addView(save, actionParams());
        panel.addView(actions, new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, dp(58)));
        dialog.setContentView(panel);
        dialog.show();
        if (dialog.getWindow() != null) {
            dialog.getWindow().setBackgroundDrawable(new ColorDrawable(Color.TRANSPARENT));
            dialog.getWindow().setLayout((int) (getResources().getDisplayMetrics().widthPixels * .94f),
                    (int) (getResources().getDisplayMetrics().heightPixels * .78f));
        }
    }

    private int dp(int value) {
        return Math.round(value * getResources().getDisplayMetrics().density);
    }

    private GradientDrawable outline(int fill, int stroke, int strokeWidth, int radius) {
        GradientDrawable drawable = new GradientDrawable();
        drawable.setColor(fill);
        drawable.setStroke(strokeWidth, stroke);
        drawable.setCornerRadius(radius);
        return drawable;
    }

    private TextView settingText(String value, int size, int color, boolean bold) {
        TextView view = new TextView(this);
        view.setText(value);
        view.setTextSize(size);
        view.setTextColor(color);
        view.setGravity(Gravity.CENTER_VERTICAL);
        view.setTypeface(bold ? android.graphics.Typeface.DEFAULT_BOLD : android.graphics.Typeface.DEFAULT);
        return view;
    }

    private void styleInput(EditText input, int size, int color) {
        input.setTextSize(size);
        input.setTextColor(color);
        input.setSingleLine(true);
        input.setPadding(0, 0, 0, 0);
        input.setBackgroundColor(Color.TRANSPARENT);
    }

    private void styleChoice(TextView view, String label, int size, boolean bold) {
        view.setText(label);
        view.setTextSize(size);
        view.setTextColor(Color.rgb(17, 17, 17));
        view.setTypeface(bold ? android.graphics.Typeface.DEFAULT_BOLD : android.graphics.Typeface.DEFAULT);
        view.setMinHeight(dp(38));
        view.setGravity(Gravity.CENTER_VERTICAL);
        if (view instanceof android.widget.CompoundButton) {
            ((android.widget.CompoundButton) view).setButtonTintList(ColorStateList.valueOf(Color.rgb(17, 17, 17)));
        }
    }

    private View settingDivider() {
        View divider = new View(this);
        divider.setBackgroundColor(Color.rgb(150, 150, 150));
        divider.setLayoutParams(new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, Math.max(1, dp(1))));
        return divider;
    }

    private Button settingButton(String label) {
        Button button = new Button(this);
        button.setText(label);
        button.setTextSize(17);
        button.setTextColor(Color.rgb(17, 17, 17));
        button.setAllCaps(false);
        button.setBackgroundColor(Color.TRANSPARENT);
        button.setPadding(0, 0, 0, 0);
        return button;
    }

    private LinearLayout.LayoutParams actionParams() {
        return new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.MATCH_PARENT, 1f);
    }

    private View actionDivider() {
        View divider = new View(this);
        divider.setBackgroundColor(Color.rgb(17, 17, 17));
        divider.setLayoutParams(new LinearLayout.LayoutParams(Math.max(1, dp(1)),
                LinearLayout.LayoutParams.MATCH_PARENT));
        return divider;
    }

    private int refreshMinutes() {
        return Math.max(1, Math.min(60, preferences.getInt("refresh_minutes", 5)));
    }

    private String normaliseBaseUrl(String value) {
        String result = value == null ? "" : value.trim();
        if (result.isEmpty()) return null;
        while (result.endsWith("/")) result = result.substring(0, result.length() - 1);
        if (result.endsWith("/api/status")) result = result.substring(0, result.length() - 11);
        return result.isEmpty() ? null : result;
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
