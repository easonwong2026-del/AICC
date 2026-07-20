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

/** V1.2 visual language with the allocation-light V2 renderer. */
final class DashboardView extends View {
    private static final int PAPER = Color.rgb(247, 247, 244);
    private static final int INK = Color.rgb(17, 17, 17);
    private static final Typeface REGULAR = Typeface.DEFAULT;
    private static final Typeface BOLD = Typeface.DEFAULT_BOLD;

    private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final RectF codexRect = new RectF();
    private final RectF workBuddyRect = new RectF();
    private final RectF deepSeekRect = new RectF();
    private final RectF systemRect = new RectF();
    private final Runnable endFlash = () -> {
        flash = false;
        invalidate();
    };

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

    void setData(DashboardData value) {
        data = value;
        data.offline = false;
        invalidate();
    }

    void setOffline(boolean offline) {
        data.offline = offline;
        invalidate();
    }

    void flashRefresh() {
        handler.removeCallbacks(endFlash);
        flash = true;
        invalidate();
        handler.postDelayed(endFlash, 800);
    }

    void settleForSleep() {
        handler.removeCallbacks(endFlash);
        flash = false;
        invalidate();
    }

    void setDeviceBattery(int percent, boolean charging, boolean visible) {
        batteryPercent = percent;
        batteryCharging = charging;
        showDeviceBattery = visible;
        invalidate();
    }

    @Override
    protected void onDraw(Canvas canvas) {
        super.onDraw(canvas);
        if (flash) {
            canvas.drawColor(INK);
            return;
        }
        canvas.drawColor(PAPER);
        float width = getWidth();
        float height = getHeight();
        float unit = Math.min(width, height);
        float margin = unit * 0.045f;
        float headerHeight = unit * 0.105f;
        drawHeader(canvas, margin, headerHeight, unit);

        float top = headerHeight + margin * 0.55f;
        float bottom = height - margin * 0.6f;
        float gap = margin * 0.45f;
        if (width > height) {
            float cardWidth = (width - margin * 2 - gap) / 2f;
            float cardHeight = (bottom - top - gap) / 2f;
            codexRect.set(margin, top, margin + cardWidth, top + cardHeight);
            workBuddyRect.set(margin + cardWidth + gap, top, width - margin, top + cardHeight);
            deepSeekRect.set(margin, top + cardHeight + gap, margin + cardWidth, bottom);
            systemRect.set(margin + cardWidth + gap, top + cardHeight + gap, width - margin, bottom);
        } else {
            float available = bottom - top - gap * 2;
            float codexHeight = available * .36f;
            float pairHeight = available * .32f;
            float splitWidth = (width - margin * 2 - gap) / 2f;
            codexRect.set(margin, top, width - margin, top + codexHeight);
            float pairTop = codexRect.bottom + gap;
            workBuddyRect.set(margin, pairTop, margin + splitWidth, pairTop + pairHeight);
            deepSeekRect.set(margin + splitWidth + gap, pairTop, width - margin, pairTop + pairHeight);
            systemRect.set(margin, workBuddyRect.bottom + gap, width - margin, bottom);
        }
        drawCodex(canvas, codexRect, unit);
        drawWorkBuddy(canvas, workBuddyRect, unit);
        drawDeepSeek(canvas, deepSeekRect, unit);
        drawSystem(canvas, systemRect, unit);
    }

    private void drawHeader(Canvas canvas, float margin, float height, float unit) {
        float box = unit * 0.06f;
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(INK);
        canvas.drawRect(margin, margin * 0.55f, margin + box, margin * 0.55f + box, paint);
        text(canvas, "AI", margin + box / 2f, margin * 0.55f + box * 0.68f,
                unit * 0.026f, PAPER, true, Paint.Align.CENTER);
        text(canvas, "COMMAND", margin + box + unit * 0.025f, margin * 0.55f + box * 0.68f,
                unit * 0.034f, INK, true, Paint.Align.LEFT);
        String state = data.offline ? "OFFLINE" : "●";
        if (data.failedCollectors > 0) state = "CHECK";
        else if (data.refreshingCollectors > 0) state = "SYNC";
        if (showDeviceBattery && batteryPercent >= 0) {
            state = batteryPercent + "%" + (batteryCharging ? " +" : "") + "   " + state;
        }
        text(canvas, state, getWidth() - margin, margin * 0.55f + box * 0.67f,
                unit * 0.024f, INK, true, Paint.Align.RIGHT);
        line(canvas, margin, height, getWidth() - margin, height, unit * 0.004f);
    }

    private void drawCodex(Canvas canvas, RectF rect, float unit) {
        card(canvas, rect, "CODEX", data.codexState, unit);
        float left = rect.left + inset(rect);
        float right = rect.right - inset(rect);
        int quotaCount = (data.fiveHourRemaining == null ? 0 : 1)
                + (data.weeklyRemaining == null ? 0 : 1);
        float footerTop = rect.bottom - rect.height() * .20f;
        if (quotaCount == 2) {
            float middle = rect.centerX();
            quotaColumn(canvas, "5小时", data.fiveHourRemaining, data.fiveHourReset,
                    left, middle - inset(rect) * .45f, rect, unit);
            line(canvas, middle, rect.top + rect.height() * .34f, middle,
                    footerTop - rect.height() * .05f, unit * .0015f);
            quotaColumn(canvas, "1周", data.weeklyRemaining, data.weeklyReset,
                    middle + inset(rect) * .45f, right, rect, unit);
        } else if (quotaCount == 1) {
            boolean weekly = data.weeklyRemaining != null;
            String label = weekly ? "1周剩余" : "5小时剩余";
            int percent = weekly ? data.weeklyRemaining : data.fiveHourRemaining;
            String reset = weekly ? data.weeklyReset : data.fiveHourReset;
            text(canvas, label, left, rect.top + rect.height() * .46f,
                    unit * .023f, INK, true, Paint.Align.LEFT);
            text(canvas, percent + "%", rect.centerX(), rect.top + rect.height() * .55f,
                    unit * .070f, INK, true, Paint.Align.CENTER);
            text(canvas, "额度重置", right, rect.top + rect.height() * .41f,
                    unit * .016f, INK, true, Paint.Align.RIGHT);
            textFitRight(canvas, reset, right, rect.top + rect.height() * .53f,
                    rect.width() * .30f, unit * .017f);
            quotaBar(canvas, left, right, rect.top + rect.height() * .63f, percent, unit);
        } else {
            text(canvas, "等待账户额度…", left, rect.top + rect.height() * .52f,
                    unit * .026f, INK, false, Paint.Align.LEFT);
        }
        line(canvas, left, footerTop, right, footerTop, unit * .0015f);
        String creditValue = data.resetCreditsProvided && data.resetCreditsCount != null
                ? data.resetCreditsCount + "次" : "未提供";
        text(canvas, "重置机会  " + creditValue, left, rect.bottom - rect.height() * .065f,
                unit * .020f, INK, true, Paint.Align.LEFT);
        String extra = data.resetCreditsProvided && !"--".equals(data.resetCreditsExpiry)
                ? "机会到期  " + data.resetCreditsExpiry
                : (data.limitBucketCount > 1 ? data.limitBucketCount + "组额度" : "");
        textFitRight(canvas, extra, right, rect.bottom - rect.height() * .065f,
                rect.width() * .50f, unit * .016f);
    }

    private void drawWorkBuddy(Canvas canvas, RectF rect, float unit) {
        String state = data.workBuddyStale ? "CACHED" : data.workBuddyState;
        card(canvas, rect, "WORKBUDDY", state, unit);
        float x = rect.left + inset(rect);
        float y = rect.top + rect.height() * .60f;
        textFit(canvas, data.workBuddyPoints, x, y, rect.width() * .70f, unit * .060f, true);
        text(canvas, "PTS", rect.right - inset(rect), y,
                unit * .019f, INK, true, Paint.Align.RIGHT);
        text(canvas, "今日使用  " + data.workBuddyUsed, x, rect.bottom - rect.height() * .11f,
                unit * .019f, INK, false, Paint.Align.LEFT);
    }

    private void drawDeepSeek(Canvas canvas, RectF rect, float unit) {
        card(canvas, rect, "DEEPSEEK API", data.deepSeekState, unit);
        float inset = inset(rect);
        float middle = rect.centerX();
        float top = rect.top + rect.height() * .43f;
        text(canvas, "余额", rect.left + inset, top, unit * .017f, INK, true, Paint.Align.LEFT);
        text(canvas, "使用", middle + inset * .55f, top, unit * .017f, INK, true, Paint.Align.LEFT);
        line(canvas, middle, rect.top + rect.height() * .33f, middle,
                rect.bottom - inset, unit * .002f);
        float valueY = rect.top + rect.height() * .68f;
        textFit(canvas, data.deepSeekBalance, rect.left + inset, valueY,
                rect.width() * .40f, unit * .044f, true);
        textFit(canvas, data.deepSeekUsage, middle + inset * .55f, valueY,
                rect.width() * .36f, unit * .044f, true);
        text(canvas, data.deepSeekCurrency, rect.right - inset, rect.bottom - rect.height() * .11f,
                unit * .018f, INK, true, Paint.Align.RIGHT);
    }

    private void drawSystem(Canvas canvas, RectF rect, float unit) {
        card(canvas, rect, "SYSTEM", data.systemState, unit);
        float x = rect.left + inset(rect);
        float y = rect.top + rect.height() * .47f;
        textFit(canvas, data.systemLabel, x, y, rect.width() - inset(rect) * 2,
                unit * .026f, false);
        textFit(canvas, "CPU " + data.cpu + "   ·   RAM " + data.ram, x,
                y + rect.height() * .22f, rect.width() - inset(rect) * 2,
                unit * .023f, true);
        if (!data.gpu.isEmpty()) {
            textFit(canvas, data.gpu, x, rect.bottom - rect.height() * .11f,
                    rect.width() * .70f, unit * .019f, false);
            textFitRight(canvas, data.updatedAt, rect.right - inset(rect),
                    rect.bottom - rect.height() * .11f, rect.width() * .27f, unit * .016f);
        } else {
            text(canvas, "SYNC  " + data.updatedAt, x, rect.bottom - rect.height() * .11f,
                    unit * .018f, INK, false, Paint.Align.LEFT);
        }
    }

    private void card(Canvas canvas, RectF rect, String title, String state, float unit) {
        paint.setStyle(Paint.Style.STROKE);
        paint.setStrokeWidth(Math.max(1.5f, unit * .0022f));
        paint.setColor(INK);
        canvas.drawRect(rect, paint);
        float inset = inset(rect);
        float baseline = rect.top + rect.height() * .19f;
        text(canvas, title, rect.left + inset, baseline, unit * .021f, INK, true, Paint.Align.LEFT);
        textFitRight(canvas, state, rect.right - inset, baseline,
                rect.width() * .42f, unit * .017f);
        line(canvas, rect.left + inset, rect.top + rect.height() * .26f,
                rect.right - inset, rect.top + rect.height() * .26f, unit * .0016f);
    }

    private void quotaColumn(Canvas canvas, String label, int percent, String reset,
                             float left, float right, RectF rect, float unit) {
        text(canvas, label, left, rect.top + rect.height() * .42f,
                unit * .019f, INK, true, Paint.Align.LEFT);
        text(canvas, percent + "%", right, rect.top + rect.height() * .43f,
                unit * .037f, INK, true, Paint.Align.RIGHT);
        quotaBar(canvas, left, right, rect.top + rect.height() * .51f, percent, unit);
        textFit(canvas, "重置 " + reset, left, rect.top + rect.height() * .68f,
                right - left, unit * .015f, false);
    }

    private void quotaBar(Canvas canvas, float left, float right, float top,
                          int percent, float unit) {
        float bottom = top + Math.max(8, unit * .012f);
        paint.setStyle(Paint.Style.STROKE);
        paint.setStrokeWidth(Math.max(1.5f, unit * .002f));
        paint.setColor(INK);
        canvas.drawRect(left, top, right, bottom, paint);
        paint.setStyle(Paint.Style.FILL);
        canvas.drawRect(left, top,
                left + (right - left) * Math.max(0, Math.min(100, percent)) / 100f,
                bottom, paint);
    }

    private float inset(RectF rect) {
        return Math.max(12, rect.width() * .045f);
    }

    private void line(Canvas canvas, float x1, float y1, float x2, float y2, float width) {
        paint.setStyle(Paint.Style.STROKE);
        paint.setStrokeWidth(Math.max(1, width));
        paint.setColor(INK);
        canvas.drawLine(x1, y1, x2, y2, paint);
    }

    private void text(Canvas canvas, String value, float x, float y, float size, int color,
                      boolean bold, Paint.Align align) {
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(color);
        paint.setTextSize(size);
        paint.setTextAlign(align);
        paint.setTypeface(bold ? BOLD : REGULAR);
        paint.setFakeBoldText(false);
        canvas.drawText(value == null ? "--" : value, x, y, paint);
    }

    private float measure(String value, float size, boolean bold) {
        paint.setTextSize(size);
        paint.setTypeface(bold ? BOLD : REGULAR);
        paint.setFakeBoldText(false);
        return paint.measureText(value == null ? "--" : value);
    }

    private float fittedSize(String value, float maxWidth, float size, boolean bold) {
        while (size > 12 && measure(value, size, bold) > maxWidth) size *= .9f;
        return size;
    }

    private void textFit(Canvas canvas, String value, float x, float y,
                         float maxWidth, float size, boolean bold) {
        text(canvas, value, x, y, fittedSize(value, maxWidth, size, bold),
                INK, bold, Paint.Align.LEFT);
    }

    private void textFitRight(Canvas canvas, String value, float x, float y,
                              float maxWidth, float size) {
        while (size > 10 && measure(value, size, true) > maxWidth) size *= .9f;
        text(canvas, value, x, y, size, INK, true, Paint.Align.RIGHT);
    }
}
