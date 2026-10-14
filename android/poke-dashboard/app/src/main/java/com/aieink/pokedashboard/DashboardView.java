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

/** Direct, allocation-light renderer mapped one-to-one to the Pencil home artboard. */
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
    private int accountPage;

    DashboardView(Context context) {
        super(context);
        data.offline = true;
        setBackgroundColor(PAPER);
        setClickable(true);
        setLongClickable(true);
        setContentDescription("Codex 额度看板，点击翻页，长按打开设置");
    }

    void setData(DashboardData value) { data = value; invalidate(); }
    void nextAccountPage() {
        if (data.accounts.size() > 4) {
            accountPage = (accountPage + 1) % ((data.accounts.size() + 3) / 4);
            invalidate();
        }
    }
    void setOffline(boolean offline) {
        if (data.offline != offline) { data.offline = offline; invalidate(); }
    }
    void setDeviceBattery(int percent, boolean charging, boolean visible) {
        batteryPercent = percent;
        batteryCharging = charging;
        showDeviceBattery = visible;
        invalidate();
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
        drawCodex(canvas);
        textFit(canvas, data.offline ? "离线 · 保留上次额度" : "同步 " + data.updatedAt, 34, 977, 500, 17, false);
        textFitRight(canvas, "长按设置", 724, 977, 150, 17);
        canvas.restore();
    }

    private void drawHeader(Canvas canvas) {
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(INK);
        canvas.drawRect(34, 20, 80, 66, paint);
        text(canvas, "AI", 57, 51, 20, PAPER, true, Paint.Align.CENTER);
        text(canvas, "COMMAND", 100, 52, 28, INK, true, Paint.Align.LEFT);
        String state = data.offline ? "OFFLINE" : "SYNC";
        if (!data.offline && !"CONNECTED".equals(data.codexState)) state = "CACHED".equals(data.codexState) ? "CACHED" : "CHECK";
        if (showDeviceBattery && batteryPercent >= 0) {
            state = batteryPercent + "%" + (batteryCharging ? " +" : "") + "  " + state;
        }
        textFitRight(canvas, state, 724, 47, 180, 17);
        filledRect(canvas, 34, 78, 690, 3, INK);
    }

    private void drawCodex(Canvas canvas) {
        outlinedRect(canvas, 34, 98, 690, 830, 2);
        text(canvas, "CODEX", 64, 151, 26, INK, true, Paint.Align.LEFT);
        textFitRight(canvas, data.codexSource, 694, 151, 210, 16);
        filledRect(canvas, 64, 179, 630, 2, INK);
        int pages = Math.max(1, (data.accounts.size() + 3) / 4);
        accountPage = Math.min(accountPage, pages - 1);
        int start = accountPage * 4;
        int count = Math.min(4, data.accounts.size() - start);
        if (count == 0) {
            text(canvas, "暂无 Codex 账号", 64, 265, 28, INK, true, Paint.Align.LEFT);
            textFit(canvas, data.codexState, 64, 315, 630, 20, false);
        }
        float rowHeight = count > 0 ? Math.min(340, 660f / count) : 0;
        for (int index = 0; index < count; index++) {
            DashboardData.Account account = data.accounts.get(start + index);
            float top = 215 + index * rowHeight;
            textFit(canvas, account.label, 64, top, 420, 20, true);
            String state = account.needsReauth ? "需要登录" : account.stale || data.offline ? "缓存" : "";
            textFitRight(canvas, state, 694, top, 150, 17);
            drawWindow(canvas, 64, top + 35, account.weekly, account.weeklyReset, "周剩余", rowHeight > 200);
            drawWindow(canvas, 394, top + 35, account.fiveHour, account.fiveHourReset, "5小时剩余", rowHeight > 200);
            if (index < count - 1) filledRect(canvas, 64, top + rowHeight - 30, 630, 1, INK);
        }
        String footer = data.totalAccounts + " 个账号";
        if (pages > 1) footer += " · " + (accountPage + 1) + "/" + pages + " · 点击翻页";
        else if (data.totalAccounts > data.accounts.size()) footer += " · 按 Mac 展示偏好";
        textFit(canvas, footer, 64, 900, 630, 17, false);
    }

    private void drawWindow(Canvas canvas, float left, float top, Integer percent, String reset, String title, boolean large) {
        text(canvas, title, left, top, 17, INK, false, Paint.Align.LEFT);
        text(canvas, percent == null ? "—" : percent + "%", left, top + (large ? 70 : 43), large ? 60 : 34,
                INK, true, Paint.Align.LEFT);
        quotaBar(canvas, left, top + (large ? 95 : 58), 285, percent == null ? 0 : percent);
        textFit(canvas, "重置 " + (hasValue(reset) ? reset : "—"), left, top + (large ? 128 : 90), 285, 14, false);
    }

    private boolean hasValue(String value) {
        return value != null && !value.isEmpty() && !"--".equals(value)
                && !"null".equalsIgnoreCase(value);
    }

    private void quotaBar(Canvas canvas, float left, float top, float width, int percent) {
        outlinedRect(canvas, left, top, width, 12, 2);
        filledRect(canvas, left + 2, top + 2, (width - 4) * Math.max(0, Math.min(100, percent)) / 100f, 8, INK);
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
        while (size > 10 && measure(value, size, bold) > maxWidth) size *= .9f;
        return size;
    }

    private void textFit(Canvas canvas, String value, float x, float y, float maxWidth,
                         float size, boolean bold) {
        text(canvas, value, x, y, fittedSize(value, maxWidth, size, bold), INK, bold, Paint.Align.LEFT);
    }

    private void textFitRight(Canvas canvas, String value, float x, float y, float maxWidth, float size) {
        text(canvas, value, x, y, fittedSize(value, maxWidth, size, true), INK, true, Paint.Align.RIGHT);
    }
}
