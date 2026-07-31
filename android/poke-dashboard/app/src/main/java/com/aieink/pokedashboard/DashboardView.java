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

    DashboardView(Context context) {
        super(context);
        setBackgroundColor(PAPER);
        setClickable(true);
        setLongClickable(true);
        setContentDescription("AI COMMAND 仪表盘，长按打开设置");
    }

    void setData(DashboardData value) { data = value; data.offline = false; invalidate(); }
    void setOffline(boolean offline) { data.offline = offline; invalidate(); }
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
        drawWorkBuddy(canvas);
        drawDeepSeek(canvas);
        drawSystem(canvas);
        canvas.restore();
    }

    private void drawHeader(Canvas canvas) {
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(INK);
        canvas.drawRect(34, 20, 80, 66, paint);
        text(canvas, "AI", 57, 51, 20, PAPER, true, Paint.Align.CENTER);
        text(canvas, "COMMAND", 100, 52, 28, INK, true, Paint.Align.LEFT);
        String state = data.offline ? "OFFLINE" : "SYNC";
        if (data.failedCollectors > 0) state = "CHECK";
        else if (data.refreshingCollectors > 0) state = "SYNC";
        if (showDeviceBattery && batteryPercent >= 0) {
            state = batteryPercent + "%" + (batteryCharging ? " +" : "") + "  " + state;
        }
        textFitRight(canvas, state, 724, 47, 180, 17);
        filledRect(canvas, 34, 78, 690, 3, INK);
    }

    private void drawCodex(Canvas canvas) {
        outlinedRect(canvas, 34, 98, 690, 315, 2);
        text(canvas, "CODEX", 64, 151, 26, INK, true, Paint.Align.LEFT);
        textFitRight(canvas, data.codexState, 694, 151, 170, 16);
        filledRect(canvas, 64, 179, 630, 2, INK);

        boolean weekly = data.weeklyRemaining != null || data.fiveHourRemaining == null;
        int percent = weekly ? valueOrZero(data.weeklyRemaining) : valueOrZero(data.fiveHourRemaining);
        String reset = weekly ? data.weeklyReset : data.fiveHourReset;
        text(canvas, weekly ? "1周剩余" : "5小时剩余", 64, 245, 24, INK, true, Paint.Align.LEFT);
        text(canvas, percent + "%", 369, 267, 60, INK, true, Paint.Align.CENTER);
        text(canvas, "额度重置", 694, 234, 20, INK, true, Paint.Align.RIGHT);
        textFitRight(canvas, reset, 694, 265, 190, 16);
        quotaBar(canvas, 64, 296, 630, percent);
        filledRect(canvas, 64, 349, 630, 2, INK);
        String credits = data.resetCreditsProvided && data.resetCreditsCount != null
                ? "重置机会  " + data.resetCreditsCount + "次" : "重置机会  未提供";
        textFit(canvas, credits, 129, 385, 210, 18, true);
        String expiry = data.resetCreditsProvided && hasValue(data.resetCreditsExpiry)
                ? "机会到期  " + data.resetCreditsExpiry : "机会到期  --";
        textFitRight(canvas, expiry, 659, 385, 270, 18);
    }

    private void drawWorkBuddy(Canvas canvas) {
        outlinedRect(canvas, 34, 429, 326, 286, 2);
        text(canvas, "WORKBUDDY", 49, 480, 26, INK, true, Paint.Align.LEFT);
        textFitRight(canvas, workBuddyStatus(), 343, 480, 142, 13);
        filledRect(canvas, 49, 503, 295, 2, INK);
        textFit(canvas, data.workBuddyPoints, 85, 599, 236, 46, true);
        text(canvas, "PTS", 321, 675, 18, INK, true, Paint.Align.RIGHT);
        textFit(canvas, "今日使用  " + data.workBuddyUsed, 81, 675, 183, 20, false);
    }

    private String workBuddyStatus() {
        String state = data.workBuddyStale ? "CACHED" : data.workBuddyState;
        if ("UNAVAILABLE".equals(state) && !data.workBuddyErrorCode.isEmpty()) {
            return state + " " + data.workBuddyErrorCode;
        }
        if (!data.workBuddyStale || data.workBuddyAgeSeconds == null) return state;
        return state + " " + compactAge(data.workBuddyAgeSeconds);
    }

    private String compactAge(long seconds) {
        if (seconds < 60) return seconds + "s";
        if (seconds < 3600) return (seconds / 60) + "m";
        if (seconds < 86400) return (seconds / 3600) + "h";
        return (seconds / 86400) + "d";
    }

    private void drawDeepSeek(Canvas canvas) {
        outlinedRect(canvas, 379, 429, 345, 286, 2);
        text(canvas, "DEEPSEEK API", 395, 481, 26, INK, true, Paint.Align.LEFT);
        textFitRight(canvas, data.deepSeekState, 697, 481, 106, 13);
        filledRect(canvas, 395, 503, 312, 2, INK);
        text(canvas, "余额", 408, 533, 17, INK, true, Paint.Align.LEFT);
        text(canvas, "使用", 573, 533, 17, INK, true, Paint.Align.LEFT);
        filledRect(canvas, 550, 516, 2, 177, INK);
        textFit(canvas, data.deepSeekBalance, 411, 611, 106, 40, true);
        textFit(canvas, data.deepSeekUsage, 613, 617, 61, 40, true);
        text(canvas, data.deepSeekCurrency, 703, 675, 18, INK, true, Paint.Align.RIGHT);
    }

    private void drawSystem(Canvas canvas) {
        outlinedRect(canvas, 34, 734, 690, 270, 2);
        text(canvas, "SYSTEM", 64, 786, 24, INK, true, Paint.Align.LEFT);
        filledRect(canvas, 64, 803, 630, 2, INK);
        textFit(canvas, data.systemLabel, 64, 864, 430, 20, false);
        textFit(canvas, "CPU " + data.cpu, 474, 865, 180, 18, true);
        textFit(canvas, "RAM " + data.ram, 474, 922, 230, 18, true);
        textFit(canvas, data.gpu, 79, 928, 330, 18, false);
        textFitRight(canvas, data.updatedAt, 694, 979, 210, 15);
    }

    private int valueOrZero(Integer value) { return value == null ? 0 : value; }

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
