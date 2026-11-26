package com.aieink.pokedashboard;

import android.app.Activity;
import android.app.Dialog;
import android.content.SharedPreferences;
import android.content.res.ColorStateList;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.GradientDrawable;
import android.text.InputFilter;
import android.text.InputType;
import android.view.Gravity;
import android.view.View;
import android.view.Window;
import android.widget.Button;
import android.widget.CheckBox;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.RadioButton;
import android.widget.RadioGroup;
import android.widget.ScrollView;
import android.widget.TextView;

import java.util.ArrayList;
import java.util.List;

/**
 * High-contrast e-ink settings modal with bordered inputs, fingerprint status,
 * layout/sort segmented controls, provider filter chips, and power options.
 */
final class SettingsDialog {
    interface Host {
        SharedPreferences prefs();
        String currentPinFor(String baseUrl);
        boolean isPaired();
        List<DashboardData.ProviderOption> availableProviders();
        int totalAccountCount();
        void onDiscoverRequested(EditText urlField, Runnable onDiscovered);
        void onPairRequested(String baseUrl, String code, Button pairButton, EditText codeField, Dialog dialog);
        void onSaveSettings(String rawUrl, String layoutMode, String sortMode, String providerFilter,
                            String accountMode, int intervalMinutes, boolean keepAwake,
                            boolean showBattery, boolean autoStart, Dialog dialog);
        void onImmediateRefresh();
    }

    private final Activity activity;
    private final Host host;

    private String selectedLayout;
    private String selectedSort;
    private String selectedProvider;
    private String selectedAccountMode;

    private SettingsDialog(Activity activity, Host host) {
        this.activity = activity;
        this.host = host;
    }

    static Dialog show(Activity activity, Host host) {
        return new SettingsDialog(activity, host).buildAndShow();
    }

    private Dialog buildAndShow() {
        SharedPreferences prefs = host.prefs();
        selectedLayout = prefs.getString(MainActivity.KEY_LAYOUT_MODE, DashboardData.LAYOUT_GRID);
        selectedSort = prefs.getString(MainActivity.KEY_SORT_MODE, DashboardData.SORT_LOWEST);
        selectedProvider = prefs.getString(MainActivity.KEY_PROVIDER_FILTER, "");
        selectedAccountMode = prefs.getString("account_mode", "all");

        Dialog dialog = new Dialog(activity);
        dialog.requestWindowFeature(Window.FEATURE_NO_TITLE);
        dialog.setCanceledOnTouchOutside(true);

        LinearLayout panel = new LinearLayout(activity);
        panel.setOrientation(LinearLayout.VERTICAL);
        panel.setBackground(boxDrawable(EinkTheme.PAPER, EinkTheme.INK, dp(3), dp(16)));
        panel.setClipToOutline(true);

        // Header
        LinearLayout header = new LinearLayout(activity);
        header.setOrientation(LinearLayout.HORIZONTAL);
        header.setGravity(Gravity.CENTER_VERTICAL);
        header.setPadding(dp(16), dp(10), dp(16), dp(10));

        TextView title = label("AICC 墨水屏设置", 20, EinkTheme.INK, true);
        header.addView(title, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f));

        boolean paired = host.isPaired();
        TextView badge = new TextView(activity);
        badge.setText(paired ? "已配对 · HTTPS" : "未配对");
        badge.setTextSize(12);
        badge.setTypeface(Typeface.DEFAULT_BOLD);
        badge.setPadding(dp(10), dp(3), dp(10), dp(3));
        if (paired) {
            badge.setTextColor(EinkTheme.PAPER);
            badge.setBackground(boxDrawable(EinkTheme.INK, EinkTheme.INK, 0, dp(6)));
        } else {
            badge.setTextColor(EinkTheme.INK);
            badge.setBackground(boxDrawable(EinkTheme.PAPER, EinkTheme.INK, dp(1), dp(6)));
        }
        header.addView(badge);
        panel.addView(header);
        panel.addView(rule(dp(2), EinkTheme.INK));

        // Scrollable content
        LinearLayout content = new LinearLayout(activity);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(dp(16), dp(10), dp(16), dp(10));

        // Section 1: Server & TLS Pairing
        content.addView(sectionTitle("1. 服务器与安全配对"));
        LinearLayout urlRow = new LinearLayout(activity);
        urlRow.setOrientation(LinearLayout.HORIZONTAL);
        urlRow.setGravity(Gravity.CENTER_VERTICAL);

        EditText urlField = borderedInput(prefs.getString(MainActivity.KEY_URL, ""),
                "https://192.168.x.x:8767", 14.5f);
        urlField.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI);
        LinearLayout.LayoutParams urlLp = new LinearLayout.LayoutParams(0, dp(38), 1f);
        urlLp.rightMargin = dp(8);
        urlRow.addView(urlField, urlLp);

        Button discoverBtn = actionPillButton("自动发现", false);
        urlRow.addView(discoverBtn, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.WRAP_CONTENT, dp(38)));
        content.addView(urlRow, rowMargins(0, dp(6)));

        TextView fpBanner = new TextView(activity);
        fpBanner.setTextSize(12f);
        fpBanner.setTextColor(EinkTheme.INK);
        fpBanner.setTypeface(Typeface.MONOSPACE);
        fpBanner.setPadding(dp(10), dp(6), dp(10), dp(6));
        fpBanner.setBackground(boxDrawable(EinkTheme.PAPER_ALT, EinkTheme.PAPER_ALT, 0, dp(6)));
        updateFingerprintBanner(fpBanner, urlField.getText().toString());
        content.addView(fpBanner, rowMargins(0, dp(6)));

        discoverBtn.setOnClickListener(v -> host.onDiscoverRequested(urlField,
                () -> updateFingerprintBanner(fpBanner, urlField.getText().toString())));

        LinearLayout pairRow = new LinearLayout(activity);
        pairRow.setOrientation(LinearLayout.HORIZONTAL);
        pairRow.setGravity(Gravity.CENTER_VERTICAL);

        EditText codeField = borderedInput("", "输入 Mac 端 8 位配对码", 14.5f);
        codeField.setInputType(InputType.TYPE_CLASS_NUMBER);
        codeField.setFilters(new InputFilter[]{new InputFilter.LengthFilter(8)});
        LinearLayout.LayoutParams codeLp = new LinearLayout.LayoutParams(0, dp(38), 1f);
        codeLp.rightMargin = dp(8);
        pairRow.addView(codeField, codeLp);

        Button pairBtn = actionPillButton("配对", true);
        pairBtn.setOnClickListener(v -> host.onPairRequested(
                urlField.getText().toString(),
                codeField.getText().toString(),
                pairBtn,
                codeField,
                dialog));
        pairRow.addView(pairBtn, new LinearLayout.LayoutParams(dp(80), dp(38)));
        content.addView(pairRow, rowMargins(0, dp(4)));

        content.addView(divider());

        // Section 2: Layout & Sort & Filter
        content.addView(sectionTitle("2. 面板布局与排序"));
        LinearLayout segRow = new LinearLayout(activity);
        segRow.setOrientation(LinearLayout.HORIZONTAL);
        LinearLayout layoutSeg = segmentedTwo("瑞士网格", "桌搭时钟",
                !DashboardData.LAYOUT_CLOCK.equals(selectedLayout),
                isFirst -> selectedLayout = isFirst ? DashboardData.LAYOUT_GRID : DashboardData.LAYOUT_CLOCK);
        LinearLayout.LayoutParams segLeftLp = new LinearLayout.LayoutParams(0, dp(36), 1f);
        segLeftLp.rightMargin = dp(8);
        segRow.addView(layoutSeg, segLeftLp);

        LinearLayout sortSeg = segmentedTwo("低额优先", "按服务商",
                !DashboardData.SORT_PROVIDER.equals(selectedSort),
                isFirst -> selectedSort = isFirst ? DashboardData.SORT_LOWEST : DashboardData.SORT_PROVIDER);
        segRow.addView(sortSeg, new LinearLayout.LayoutParams(0, dp(36), 1f));
        content.addView(segRow, rowMargins(0, dp(8)));

        content.addView(sectionTitle("显示服务商与数量"));
        content.addView(buildProviderChips(), rowMargins(0, dp(6)));

        int modeIndex = "one".equals(selectedAccountMode) ? 1 : "two".equals(selectedAccountMode) ? 2 : 0;
        LinearLayout countSeg = segmentedThree("全部账号", "仅 1 个", "仅 2 个", modeIndex,
                idx -> selectedAccountMode = idx == 1 ? "one" : idx == 2 ? "two" : "all");
        content.addView(countSeg, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, dp(34)));
        content.addView(divider());

        // Section 3: Refresh & Power
        content.addView(sectionTitle("3. 刷新与电源"));
        LinearLayout intervalRow = new LinearLayout(activity);
        intervalRow.setOrientation(LinearLayout.HORIZONTAL);
        intervalRow.setGravity(Gravity.CENTER_VERTICAL);
        TextView intervalLabel = label("同步间隔（分钟）", 14.5f, EinkTheme.INK, true);
        intervalRow.addView(intervalLabel);

        EditText intervalField = borderedInput(
                String.valueOf(Math.max(1, Math.min(60, prefs.getInt("refresh_minutes", 5)))),
                "5", 14.5f);
        intervalField.setGravity(Gravity.CENTER);
        intervalField.setInputType(InputType.TYPE_CLASS_NUMBER);
        LinearLayout.LayoutParams intLp = new LinearLayout.LayoutParams(dp(68), dp(34));
        intLp.leftMargin = dp(8);
        intLp.rightMargin = dp(8);
        intervalRow.addView(intervalField, intLp);
        intervalRow.addView(label("唤醒时立即同步", 13f, EinkTheme.MUTED, false));
        content.addView(intervalRow, rowMargins(0, dp(4)));

        RadioGroup displayGroup = new RadioGroup(activity);
        displayGroup.setOrientation(RadioGroup.VERTICAL);
        RadioButton screenSaverRadio = new RadioButton(activity);
        styleChoice(screenSaverRadio, "省电屏保：允许休眠，唤醒更新（推荐）", 14f, true);
        RadioButton alwaysOnRadio = new RadioButton(activity);
        styleChoice(alwaysOnRadio, "桌面常亮：保持亮屏并按间隔自动刷新", 14f, false);
        displayGroup.addView(screenSaverRadio);
        displayGroup.addView(alwaysOnRadio);
        boolean keepAwake = prefs.getBoolean(MainActivity.KEY_KEEP_AWAKE, false);
        screenSaverRadio.setChecked(!keepAwake);
        alwaysOnRadio.setChecked(keepAwake);
        content.addView(displayGroup);

        LinearLayout checkRow = new LinearLayout(activity);
        checkRow.setOrientation(LinearLayout.HORIZONTAL);
        CheckBox showBattery = new CheckBox(activity);
        styleChoice(showBattery, "底栏/顶栏显示电量", 14f, false);
        showBattery.setChecked(prefs.getBoolean(MainActivity.KEY_SHOW_BATTERY, true));
        LinearLayout.LayoutParams cbLp = new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f);
        checkRow.addView(showBattery, cbLp);

        CheckBox autoStart = new CheckBox(activity);
        styleChoice(autoStart, "开机自动启动", 14f, false);
        autoStart.setChecked(prefs.getBoolean("auto_start", true));
        checkRow.addView(autoStart, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f));
        content.addView(checkRow);

        ScrollView scroll = new ScrollView(activity);
        scroll.setFillViewport(true);
        scroll.addView(content);
        panel.addView(scroll, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, 0, 1f));

        // Bottom 3-column action bar
        panel.addView(rule(dp(2), EinkTheme.INK));
        LinearLayout actions = new LinearLayout(activity);
        actions.setOrientation(LinearLayout.HORIZONTAL);

        Button cancelBtn = footerButton("取消", false, true, false);
        cancelBtn.setOnClickListener(v -> dialog.dismiss());
        Button refreshBtn = footerButton("立即刷新", false, false, false);
        refreshBtn.setOnClickListener(v -> {
            dialog.dismiss();
            host.onImmediateRefresh();
        });
        Button saveBtn = footerButton("保存设置", true, false, true);
        saveBtn.setOnClickListener(v -> {
            int minutes = 5;
            try {
                minutes = Integer.parseInt(intervalField.getText().toString().trim());
            } catch (NumberFormatException ignored) { }
            minutes = Math.max(1, Math.min(60, minutes));
            host.onSaveSettings(
                    urlField.getText().toString(),
                    selectedLayout,
                    selectedSort,
                    selectedProvider,
                    selectedAccountMode,
                    minutes,
                    alwaysOnRadio.isChecked(),
                    showBattery.isChecked(),
                    autoStart.isChecked(),
                    dialog);
        });

        actions.addView(cancelBtn, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.MATCH_PARENT, 1f));
        actions.addView(verticalRule());
        actions.addView(refreshBtn, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.MATCH_PARENT, 1f));
        actions.addView(verticalRule());
        actions.addView(saveBtn, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.MATCH_PARENT, 1f));
        panel.addView(actions, new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, dp(48)));

        dialog.setContentView(panel);
        dialog.show();
        if (dialog.getWindow() != null) {
            dialog.getWindow().setBackgroundDrawable(new ColorDrawable(Color.TRANSPARENT));
            int w = (int) (activity.getResources().getDisplayMetrics().widthPixels * 0.93f);
            int h = (int) (activity.getResources().getDisplayMetrics().heightPixels * 0.91f);
            dialog.getWindow().setLayout(w, h);
        }
        return dialog;
    }

    private void updateFingerprintBanner(TextView banner, String rawUrl) {
        String base = DiscoveryClient.normaliseBaseUrl(rawUrl);
        String pin = base == null ? null : host.currentPinFor(base);
        if (pin != null && pin.length() >= 12) {
            String shortPin = pin.substring(0, 4) + " " + pin.substring(4, 8) + " " + pin.substring(8, 12) + " …";
            banner.setText("指纹：" + shortPin + " · TLS 1.3 · 仅可信局域网");
        } else {
            banner.setText("点击「自动发现」读取 Mac 证书指纹 · 仅在可信局域网配对");
        }
    }

    private View buildProviderChips() {
        List<DashboardData.ProviderOption> providers = host.availableProviders();
        LinearLayout container = new LinearLayout(activity);
        container.setOrientation(LinearLayout.VERTICAL);

        final List<TextView> chips = new ArrayList<>();
        final List<String> chipValues = new ArrayList<>();

        Runnable refreshChips = () -> {
            for (int i = 0; i < chips.size(); i++) {
                boolean active = chipValues.get(i).equalsIgnoreCase(selectedProvider);
                styleChip(chips.get(i), active);
            }
        };

        List<String> labels = new ArrayList<>();
        labels.add("全部 (" + host.totalAccountCount() + ")");
        chipValues.add("");
        for (DashboardData.ProviderOption opt : providers) {
            labels.add(opt.providerName + " (" + opt.count + ")");
            chipValues.add(opt.provider);
        }

        // Pack up to 3 chips per row cleanly for narrow e-ink widths.
        LinearLayout row = null;
        for (int i = 0; i < labels.size(); i++) {
            if (i % 3 == 0) {
                row = new LinearLayout(activity);
                row.setOrientation(LinearLayout.HORIZONTAL);
                LinearLayout.LayoutParams rlp = new LinearLayout.LayoutParams(
                        LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT);
                if (i > 0) rlp.topMargin = dp(6);
                container.addView(row, rlp);
            }
            final int index = i;
            TextView chip = new TextView(activity);
            chip.setText(labels.get(i));
            chip.setTextSize(13.5f);
            chip.setTypeface(Typeface.DEFAULT_BOLD);
            chip.setGravity(Gravity.CENTER);
            chip.setPadding(dp(12), dp(5), dp(12), dp(5));
            chip.setMaxLines(1);
            chip.setOnClickListener(v -> {
                selectedProvider = chipValues.get(index);
                refreshChips.run();
            });
            chips.add(chip);
            LinearLayout.LayoutParams clp = new LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.WRAP_CONTENT, LinearLayout.LayoutParams.WRAP_CONTENT);
            clp.rightMargin = dp(8);
            row.addView(chip, clp);
        }
        refreshChips.run();
        return container;
    }

    private void styleChip(TextView chip, boolean active) {
        if (active) {
            chip.setTextColor(EinkTheme.PAPER);
            chip.setBackground(boxDrawable(EinkTheme.INK, EinkTheme.INK, dp(1), dp(20)));
        } else {
            chip.setTextColor(EinkTheme.INK);
            chip.setBackground(boxDrawable(EinkTheme.PAPER, EinkTheme.INK, dp(1), dp(20)));
        }
    }

    private interface SegCallback {
        void onSelect(boolean firstSelected);
    }

    private interface TriCallback {
        void onSelect(int index);
    }

    private LinearLayout segmentedTwo(String first, String second, boolean firstActive, SegCallback cb) {
        LinearLayout box = new LinearLayout(activity);
        box.setOrientation(LinearLayout.HORIZONTAL);
        box.setBackground(boxDrawable(EinkTheme.PAPER, EinkTheme.INK, dp(2), dp(8)));
        box.setPadding(dp(2), dp(2), dp(2), dp(2));

        TextView a = segButton(first);
        TextView b = segButton(second);
        final boolean[] state = {firstActive};
        styleSeg(a, state[0]);
        styleSeg(b, !state[0]);
        a.setOnClickListener(v -> {
            state[0] = true;
            styleSeg(a, true);
            styleSeg(b, false);
            cb.onSelect(true);
        });
        b.setOnClickListener(v -> {
            state[0] = false;
            styleSeg(a, false);
            styleSeg(b, true);
            cb.onSelect(false);
        });
        box.addView(a, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.MATCH_PARENT, 1f));
        box.addView(b, new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.MATCH_PARENT, 1f));
        return box;
    }

    private LinearLayout segmentedThree(String s0, String s1, String s2, int initial, TriCallback cb) {
        LinearLayout box = new LinearLayout(activity);
        box.setOrientation(LinearLayout.HORIZONTAL);
        box.setBackground(boxDrawable(EinkTheme.PAPER, EinkTheme.INK, dp(2), dp(8)));
        box.setPadding(dp(2), dp(2), dp(2), dp(2));

        TextView[] items = {segButton(s0), segButton(s1), segButton(s2)};
        for (int i = 0; i < items.length; i++) {
            final int idx = i;
            styleSeg(items[i], i == initial);
            items[i].setOnClickListener(v -> {
                for (int j = 0; j < items.length; j++) styleSeg(items[j], j == idx);
                cb.onSelect(idx);
            });
            box.addView(items[i], new LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.MATCH_PARENT, 1f));
        }
        return box;
    }

    private TextView segButton(String text) {
        TextView tv = new TextView(activity);
        tv.setText(text);
        tv.setTextSize(14);
        tv.setTypeface(Typeface.DEFAULT_BOLD);
        tv.setGravity(Gravity.CENTER);
        return tv;
    }

    private void styleSeg(TextView tv, boolean active) {
        if (active) {
            tv.setTextColor(EinkTheme.PAPER);
            tv.setBackground(boxDrawable(EinkTheme.INK, EinkTheme.INK, 0, dp(6)));
        } else {
            tv.setTextColor(EinkTheme.INK);
            tv.setBackgroundColor(Color.TRANSPARENT);
        }
    }

    private TextView sectionTitle(String text) {
        TextView tv = label(text, 13, EinkTheme.MUTED, true);
        tv.setPadding(0, 0, 0, dp(6));
        return tv;
    }

    private TextView label(String text, float size, int color, boolean bold) {
        TextView tv = new TextView(activity);
        tv.setText(text);
        tv.setTextSize(size);
        tv.setTextColor(color);
        tv.setTypeface(bold ? Typeface.DEFAULT_BOLD : Typeface.DEFAULT);
        tv.setGravity(Gravity.CENTER_VERTICAL);
        return tv;
    }

    private EditText borderedInput(String value, String hint, float size) {
        EditText et = new EditText(activity);
        et.setText(value);
        et.setHint(hint);
        et.setTextSize(size);
        et.setTextColor(EinkTheme.INK);
        et.setHintTextColor(EinkTheme.DISABLED);
        et.setSingleLine(true);
        et.setPadding(dp(10), dp(6), dp(10), dp(6));
        et.setBackground(boxDrawable(EinkTheme.WHITE, EinkTheme.INK, dp(1) + 1, dp(8)));
        return et;
    }

    private Button actionPillButton(String text, boolean solid) {
        Button btn = new Button(activity);
        btn.setText(text);
        btn.setTextSize(14.5f);
        btn.setTypeface(Typeface.DEFAULT_BOLD);
        btn.setAllCaps(false);
        btn.setPadding(dp(12), 0, dp(12), 0);
        if (solid) {
            btn.setTextColor(EinkTheme.PAPER);
            btn.setBackground(boxDrawable(EinkTheme.INK, EinkTheme.INK, dp(2), dp(8)));
        } else {
            btn.setTextColor(EinkTheme.INK);
            btn.setBackground(boxDrawable(EinkTheme.PAPER, EinkTheme.INK, dp(2), dp(8)));
        }
        return btn;
    }

    private Button footerButton(String text, boolean primary, boolean roundBottomLeft, boolean roundBottomRight) {
        Button btn = new Button(activity);
        btn.setText(text);
        btn.setTextSize(15.5f);
        btn.setTypeface(Typeface.DEFAULT_BOLD);
        btn.setAllCaps(false);
        btn.setTextColor(primary ? EinkTheme.PAPER : EinkTheme.INK);
        float r = dp(13);
        GradientDrawable gd = new GradientDrawable();
        gd.setColor(primary ? EinkTheme.INK : EinkTheme.PAPER);
        gd.setCornerRadii(new float[]{
                0f, 0f, 0f, 0f,
                roundBottomRight ? r : 0f, roundBottomRight ? r : 0f,
                roundBottomLeft ? r : 0f, roundBottomLeft ? r : 0f
        });
        btn.setBackground(gd);
        return btn;
    }

    private void styleChoice(TextView view, String labelText, float size, boolean bold) {
        view.setText(labelText);
        view.setTextSize(size);
        view.setTextColor(EinkTheme.INK);
        view.setTypeface(bold ? Typeface.DEFAULT_BOLD : Typeface.DEFAULT);
        view.setMinHeight(dp(28));
        view.setPadding(view.getPaddingLeft(), dp(2), view.getPaddingRight(), dp(2));
        view.setGravity(Gravity.CENTER_VERTICAL);
        if (view instanceof android.widget.CompoundButton) {
            ((android.widget.CompoundButton) view).setButtonTintList(ColorStateList.valueOf(EinkTheme.INK));
        }
    }

    private View divider() {
        View v = new View(activity);
        v.setBackgroundColor(EinkTheme.HAIR);
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, Math.max(1, dp(1)));
        lp.topMargin = dp(8);
        lp.bottomMargin = dp(6);
        v.setLayoutParams(lp);
        return v;
    }

    private View rule(int heightPx, int color) {
        View v = new View(activity);
        v.setBackgroundColor(color);
        v.setLayoutParams(new LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, heightPx));
        return v;
    }

    private View verticalRule() {
        View v = new View(activity);
        v.setBackgroundColor(EinkTheme.INK);
        v.setLayoutParams(new LinearLayout.LayoutParams(Math.max(1, dp(1) + 1),
                LinearLayout.LayoutParams.MATCH_PARENT));
        return v;
    }

    private LinearLayout.LayoutParams rowMargins(int top, int bottom) {
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, LinearLayout.LayoutParams.WRAP_CONTENT);
        lp.topMargin = top;
        lp.bottomMargin = bottom;
        return lp;
    }

    private GradientDrawable boxDrawable(int fill, int stroke, int strokeWidth, int radius) {
        GradientDrawable gd = new GradientDrawable();
        gd.setColor(fill);
        if (strokeWidth > 0) gd.setStroke(strokeWidth, stroke);
        gd.setCornerRadius(radius);
        return gd;
    }

    private int dp(int value) {
        return Math.round(value * activity.getResources().getDisplayMetrics().density);
    }
}
