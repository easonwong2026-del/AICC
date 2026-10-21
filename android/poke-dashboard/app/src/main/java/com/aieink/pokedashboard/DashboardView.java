package com.aieink.pokedashboard;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.RectF;
import android.graphics.Typeface;
import android.os.Handler;
import android.os.Looper;
import android.view.View;

/**
 * Direct, allocation-light renderer for the Poke4S e-ink panel.  Four quota
 * cards a page, any provider, paged with a tap; only real changes repaint.
 */
final class DashboardView extends View {
    private static final int PAPER = Color.rgb(247, 247, 244);
    private static final int INK = Color.rgb(17, 17, 17);
    private static final float DESIGN_WIDTH = 758f;
    private static final float DESIGN_HEIGHT = 1024f;
    private static final Typeface REGULAR = Typeface.DEFAULT;
    private static final Typeface BOLD = Typeface.DEFAULT_BOLD;

    private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final RectF rect = new RectF();
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final Runnable endFlash = () -> { flash = false; invalidate(); };

    private DashboardData data = new DashboardData();
    private boolean flash;
    private int batteryPercent = -1;
    private boolean batteryCharging;
    private boolean showDeviceBattery = true;

    DashboardView(Context context) {
        super(context);
        data.offline = true;
        setBackgroundColor(PAPER);
        setClickable(true);
        setLongClickable(true);
        setContentDescription("AICC 额度看板，点击翻页，长按打开设置");
    }

    void setData(DashboardData value) {
        value.clampPage();
        data = value;
        invalidate();
    }

    void nextPage() {
        if (data.pageCount() > 1) {
            data.nextPage();
            invalidate();
        }
    }

    void setOffline(boolean offline) {
        if (data.offline != offline) { data.offline = offline; invalidate(); }
    }

    void setDeviceBattery(int percent, boolean charging, boolean visible) {
        if (batteryPercent == percent && batteryCharging == charging && showDeviceBattery == visible) return;
        batteryPercent = percent;
        batteryCharging = charging;
        showDeviceBattery = visible;
        invalidate();
    }

    void updateSyncTime(String updatedAt) {
        String value = updatedAt == null || updatedAt.isEmpty() ? "--" : updatedAt;
        if (!value.equals(data.updatedAt)) { data.updatedAt = value; invalidate(); }
    }

    void flashRefresh() {
        handler.removeCallbacks(endFlash);
        flash = true;
        invalidate();
        handler.postDelayed(endFlash, 800);
    }

    void settleForSleep() { handler.removeCallbacks(endFlash); flash = false; invalidate(); }

    @Override protected void onDraw(Canvas canvas) {
        super.onDraw(canvas);
        if (flash) { canvas.drawColor(INK); return; }
        canvas.drawColor(PAPER);
        float sx = getWidth() / DESIGN_WIDTH;
        float sy = getHeight() / DESIGN_HEIGHT;
        canvas.save();
        canvas.scale(sx, sy);
        drawHeader(canvas);
        drawQuota(canvas);
        textFit(canvas, data.offline ? "离线 · 保留上次额度" : "同步 " + data.updatedAt, 34, 977, 470, 17, false);
        textFitRight(canvas, "长按设置", 724, 977, 150, 17);
        canvas.restore();
    }

    private void drawHeader(Canvas canvas) {
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(INK);
        canvas.drawRect(34, 20, 80, 66, paint);
        text(canvas, "AI", 57, 51, 20, PAPER, true, Paint.Align.CENTER);
        text(canvas, "COMMAND", 100, 52, 28, INK, true, Paint.Align.LEFT);
        String state = headerState();
        if (showDeviceBattery && batteryPercent >= 0) {
            state = batteryPercent + "%" + (batteryCharging ? " +" : "") + "  " + state;
        }
        textFitRight(canvas, state, 724, 47, 190, 17);
        filledRect(canvas, 34, 78, 690, 3, INK);
    }

    private String headerState() {
        if (data.offline) return "OFFLINE";
        if (data.accounts.isEmpty()) return "EMPTY";
        if (!"Connected".equals(data.state)) return "Cached".equals(data.state) ? "CACHED" : "CHECK";
        return "SYNC";
    }

    private void drawQuota(Canvas canvas) {
        outlinedRect(canvas, 34, 98, 690, 830, 2);
        int total = data.shownAccounts().size();
        text(canvas, "QUOTA", 64, 151, 26, INK, true, Paint.Align.LEFT);
        textFitRight(canvas, total + " 张 · " + providerSummary(), 694, 151, 430, 16);
        filledRect(canvas, 64, 179, 630, 2, INK);

        data.clampPage();
        java.util.List<DashboardData.Account> visible = data.visibleAccounts();
        if (visible.isEmpty()) {
            text(canvas, "Magpie 暂无额度", 64, 268, 28, INK, true, Paint.Align.LEFT);
            textFit(canvas, data.offline ? "离线，等待重新连接" : "打开 Magpie 检查服务商登录", 64, 312, 630, 19, false);
        }
        int count = visible.size();
        float rowHeight = count > 0 ? Math.min(320, 660f / count) : 0;
        for (int index = 0; index < count; index++) {
            drawCard(canvas, visible.get(index), 205 + index * rowHeight, rowHeight);
            if (index < count - 1) filledRect(canvas, 64, 205 + (index + 1) * rowHeight - 22, 630, 1, INK);
        }

        String footer = total + " 张额度卡";
        if (data.pageCount() > 1) {
            footer += " · " + (data.page + 1) + "/" + data.pageCount() + " · 点击翻页";
        } else if (data.accounts.size() > total) {
            footer += " · 按设置只显示 " + total + " 张";
        }
        textFit(canvas, footer, 64, 900, 630, 17, false);
    }

    private String providerSummary() {
        StringBuilder summary = new StringBuilder();
        for (DashboardData.Account account : data.shownAccounts()) {
            if (summary.indexOf(account.providerName) >= 0) continue;
            if (summary.length() > 0) summary.append(" / ");
            summary.append(account.providerName);
            if (summary.length() > 48) break;
        }
        return summary.length() == 0 ? "—" : summary.toString();
    }

    private void drawCard(Canvas canvas, DashboardData.Account account, float top, float height) {
        textFit(canvas, account.title(), 64, top + 24, 430, 21, true);
        String status = account.statusText();
        if (status != null) textFitRight(canvas, status, 694, top + 24, 130, 16);
        else if (account.plan != null) textFitRight(canvas, account.plan, 694, top + 24, 200, 15);

        boolean roomy = height >= 150;
        if (account.isBalance()) {
            textFit(canvas, account.balanceText(), 64, top + (roomy ? 88 : 74), 420, roomy ? 46 : 38, true);
            textFit(canvas, "余额", 64, top + (roomy ? 120 : 102), 200, 15, false);
            return;
        }
        DashboardData.Item primary = account.primary();
        if (primary == null) {
            textFit(canvas, "无额度", 64, top + (roomy ? 84 : 70), 420, 24, true);
            return;
        }
        textFit(canvas, primary.primaryText(), 64, top + (roomy ? 90 : 74), 330, roomy ? 44 : 36, true);
        if (primary.isPercent()) {
            textFit(canvas, primary.name, 64, top + (roomy ? 120 : 100), 330, 16, false);
        } else if (primary.unitLabel() != null) {
            textFit(canvas, primary.unitLabel(), 64, top + (roomy ? 120 : 100), 330, 15, false);
        }
        DashboardData.Item secondary = account.secondary();
        if (secondary != null) {
            textFitRight(canvas, secondary.name + " " + secondary.primaryText(), 694,
                    top + (roomy ? 86 : 72), 300, 18);
            if (account.items.size() > 2) {
                textFitRight(canvas, "+" + (account.items.size() - 2) + " 个窗口", 694,
                        top + (roomy ? 112 : 96), 300, 14);
            }
        }
        String reset = primary.reset != null ? primary.reset
                : (secondary != null ? secondary.reset : null);
        textFit(canvas, "重置 " + (reset == null ? "未知" : reset), 64,
                top + (roomy ? 148 : 124), 430, 15, false);
    }

    private void outlinedRect(Canvas canvas, float x, float y, float width, float height, float stroke) {
        paint.setStyle(Paint.Style.STROKE);
        paint.setStrokeWidth(stroke);
        paint.setColor(INK);
        rect.set(x, y, x + width, y + height);
        canvas.drawRect(rect, paint);
    }

    private void filledRect(Canvas canvas, float x, float y, float width, float height, int color) {
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(color);
        canvas.drawRect(x, y, x + width, y + height, paint);
    }

    private void text(Canvas canvas, String value, float x, float y, float size, int color,
                      boolean bold, Paint.Align align) {
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(color);
        paint.setTextSize(size);
        paint.setTextAlign(align);
        paint.setTypeface(bold ? BOLD : REGULAR);
        paint.setFakeBoldText(false);
        canvas.drawText(value == null || value.isEmpty() ? "--" : value, x, y, paint);
    }

    private float measure(String value, float size, boolean bold) {
        paint.setTextSize(size);
        paint.setTypeface(bold ? BOLD : REGULAR);
        return paint.measureText(value == null || value.isEmpty() ? "--" : value);
    }

    private float fittedSize(String value, float maxWidth, float size, boolean bold) {
        float current = size;
        while (current > 9 && measure(value, current, bold) > maxWidth) current *= .92f;
        return current;
    }

    private void textFit(Canvas canvas, String value, float x, float y, float maxWidth,
                         float size, boolean bold) {
        text(canvas, value, x, y, fittedSize(value, maxWidth, size, bold), INK, bold, Paint.Align.LEFT);
    }

    private void textFitRight(Canvas canvas, String value, float x, float y, float maxWidth, float size) {
        text(canvas, value, x, y, fittedSize(value, maxWidth, size, true), INK, true, Paint.Align.RIGHT);
    }
}
