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
 * Direct, allocation-light renderer for the Poke4S e-ink panel.
 *
 * <p>Two kinds of page: an overview that leads with the window closest to
 * running out and lists three more accounts, then one page per account that
 * shows every window that account has — including the ones the overview had to
 * leave out.  A tap moves to the next page, a long-press opens settings.
 *
 * <p>Layout follows the visual language borrowed from magpie-usage-display:
 * small label above, oversized number with a superscript unit, a thick rounded
 * bar, then a relative countdown.  Colour is not available on this panel, so
 * hierarchy comes from weight and grey instead.
 */
final class DashboardView extends View {
    static final int SUMMARY_ROWS = 3;

    private static final int PAPER = Color.rgb(247, 247, 244);
    private static final int INK = Color.rgb(17, 17, 17);
    private static final int MUTED = Color.rgb(95, 94, 90);
    private static final int HAIR = Color.rgb(201, 199, 191);
    private static final int TRACK = Color.rgb(226, 224, 216);

    private static final float DESIGN_WIDTH = 758f;
    private static final float DESIGN_HEIGHT = 1024f;
    private static final float LEFT = 34f;
    private static final float RIGHT = 724f;
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
        DashboardData.Account detail = data.detailAccount();
        if (data.accounts.isEmpty()) {
            drawEmpty(canvas);
        } else if (detail != null) {
            drawDetail(canvas, detail);
        } else {
            drawOverview(canvas);
        }
        drawFooter(canvas);
        canvas.restore();
    }

    // ---------------------------------------------------------------- header

    private void drawHeader(Canvas canvas) {
        filledRect(canvas, LEFT, 26, 44, 44, INK);
        text(canvas, "AI", LEFT + 22, 56, 22, PAPER, true, Paint.Align.CENTER);
        text(canvas, "AICC", 92, 58, 34, INK, true, Paint.Align.LEFT);
        text(canvas, "MAGPIE", 92, 84, 18, MUTED, true, Paint.Align.LEFT);

        String state = headerState();
        float size = fittedSize(state, 300, 24, true);
        paint.setTextSize(size);
        paint.setTypeface(BOLD);
        float width = paint.measureText(state);
        float dotX = RIGHT - width - 28;
        if (data.offline) {
            paint.setStyle(Paint.Style.STROKE);
            paint.setStrokeWidth(2.5f);
            paint.setColor(MUTED);
            canvas.drawCircle(dotX + 6, 50, 6, paint);
        } else {
            paint.setStyle(Paint.Style.FILL);
            paint.setColor(INK);
            canvas.drawCircle(dotX + 6, 50, 6, paint);
        }
        text(canvas, state, RIGHT, 58, 24, MUTED, true, Paint.Align.RIGHT);
        filledRect(canvas, LEFT, 100, RIGHT - LEFT, 2, INK);
    }

    private String headerState() {
        if (data.offline) return "OFFLINE";
        if (data.accounts.isEmpty()) return "EMPTY";
        if (!"Connected".equals(data.state)) return "Cached".equals(data.state) ? "CACHED" : "CHECK";
        return "SYNC · " + clock(data.updatedAt);
    }

    /** The panel has room for HH:mm, not for a full date; the date is on the card. */
    private static String clock(String stamp) {
        String text = stamp == null ? "" : stamp.trim();
        if (text.length() >= 5) {
            String tail = text.substring(text.length() - 5);
            if (tail.matches("\\d{2}:\\d{2}")) return tail;
        }
        return text.isEmpty() ? "--" : text;
    }

    // -------------------------------------------------------------- overview

    private void drawOverview(Canvas canvas) {
        DashboardData.Slot hero = data.hero();
        if (hero == null) { drawEmpty(canvas); return; }

        outlinedRect(canvas, LEFT, 116, RIGHT - LEFT, 334, 3, 10);
        textFit(canvas, "最低剩余 · " + hero.account.providerName + " " + hero.item.label(),
                LEFT + 26, 164, 600, 26, false);
        drawHeroValue(canvas, hero, 60, 316);

        rect.set(60, 352, 698, 368);
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(TRACK);
        canvas.drawRoundRect(rect, 8, 8, paint);
        double share = hero.item.remaining == null ? 0 : hero.item.remaining;
        float filled = (float) (638 * Math.min(1, Math.max(0, share / 100)));
        if (filled > 0) {
            rect.set(60, 352, 60 + Math.max(8, filled), 368);
            paint.setColor(INK);
            canvas.drawRoundRect(rect, 8, 8, paint);
        }

        String caption = caption(hero);
        textFit(canvas, caption, 60, 414, 620, 24, false);

        java.util.List<DashboardData.Account> rows = data.summaryAccounts();
        int count = Math.min(SUMMARY_ROWS, rows.size());
        for (int index = 0; index < count; index++) {
            drawSummaryRow(canvas, rows.get(index), 476 + index * 140);
        }
    }

    private void drawHeroValue(Canvas canvas, DashboardData.Slot slot, float x, float baseline) {
        DashboardData.Item item = slot.item;
        String value = item.primaryText();
        String unit = item.unitLabel();
        if (value.endsWith("%")) { value = value.substring(0, value.length() - 1); unit = "%"; }
        float numberWidth = measure(value, 150, true);
        float size = 150;
        // A credits count can be long; keep the number inside the card.
        while (size > 90 && numberWidth + 60 > 600) {
            size *= 0.9f;
            numberWidth = measure(value, size, true);
        }
        text(canvas, value, x, baseline, size, INK, true, Paint.Align.LEFT);
        if (unit != null) text(canvas, unit, x + numberWidth + 10, baseline, 44, MUTED, true, Paint.Align.LEFT);
    }

    private String caption(DashboardData.Slot slot) {
        DashboardData.Item item = slot.item;
        StringBuilder text = new StringBuilder();
        String countdown = item.countdown(nowSeconds());
        if (countdown != null) text.append(countdown);
        if (slot.account.items.size() == 2) {
            DashboardData.Item second = slot.account.items.get(1);
            if (text.length() > 0) text.append(" · ");
            text.append(second.label()).append(" ").append(second.primaryText());
        } else if (slot.account.items.size() > 2) {
            if (text.length() > 0) text.append(" · ");
            text.append("共 ").append(slot.account.items.size()).append(" 个窗口");
        }
        if (text.length() == 0) text.append(item.name);
        return text.toString();
    }

    private void drawSummaryRow(Canvas canvas, DashboardData.Account account, float top) {
        textFit(canvas, account.title(), LEFT, top + 34, 470, 28, true);
        String right = account.statusText() != null ? account.statusText()
                : (account.plan == null ? "" : account.plan);
        if (!right.isEmpty()) textFitRight(canvas, right, RIGHT, top + 34, 220, 22);

        DashboardData.Item item = account.primary();
        String value = account.isBalance() ? account.balanceText() : (item == null ? "无额度" : item.primaryText());
        float valueSize = account.isBalance() ? 40 : 52;
        text(canvas, value, LEFT, top + 96, valueSize, INK, true, Paint.Align.LEFT);
        float width = measure(value, valueSize, true);
        String unit = null;
        if (account.isBalance()) unit = "余额";
        else if (item != null && item.isPercent()) unit = item.label();
        else if (item != null) unit = item.unitLabel();
        if (unit != null) text(canvas, unit, LEFT + width + 12, top + 96, 24, MUTED, false, Paint.Align.LEFT);

        String detail = trailing(account);
        if (detail != null) textFitRight(canvas, detail, RIGHT, top + 96, 240, 22);

        float share = share(account);
        float barWidth = 300;
        bar(canvas, LEFT, top + 112, barWidth, 6, share);
        String countdown = item == null ? null : item.countdown(nowSeconds());
        if (countdown != null) textFitRight(canvas, countdown, RIGHT, top + 120, 320, 20);
    }

    private String trailing(DashboardData.Account account) {
        if (account.isBalance()) return null;
        DashboardData.Item item = account.primary();
        if (item == null) return null;
        if (account.items.size() == 2) {
            DashboardData.Item second = account.items.get(1);
            return second.label() + " " + second.primaryText();
        }
        if (account.items.size() > 2) return "+" + (account.items.size() - 1) + " 窗口";
        return item.totalText();
    }

    private static float share(DashboardData.Account account) {
        DashboardData.Item item = account.primary();
        if (account.isBalance() || item == null || item.remaining == null || item.unlimited) return -1;
        return (float) Math.min(1, Math.max(0, item.remaining / 100));
    }

    // ---------------------------------------------------------------- detail

    private void drawDetail(Canvas canvas, DashboardData.Account account) {
        textFit(canvas, account.title(), LEFT, 152, 520, 40, true);
        if (account.plan != null) textFitRight(canvas, account.plan, RIGHT, 152, 200, 24);
        String sub = account.items.isEmpty()
                ? "余额 · " + account.balanceText()
                : account.items.size() + " 个额度窗口";
        if (account.statusText() != null) sub = account.statusText() + " · " + sub;
        textFit(canvas, sub, LEFT, 190, 640, 24, false);
        filledRect(canvas, LEFT, 208, RIGHT - LEFT, 2, INK);

        if (account.items.isEmpty()) {
            text(canvas, account.balanceText(), LEFT, 300, 84, INK, true, Paint.Align.LEFT);
            return;
        }

        int count = account.items.size();
        float pitch = Math.min(122, 664f / count);
        float start = 226 + Math.max(0, (664 - count * pitch) / 2);
        for (int index = 0; index < count; index++) {
            DashboardData.Item item = account.items.get(index);
            float top = start + index * pitch;
            textFit(canvas, item.label(), LEFT, top + 30, 380, 28, true);
            String value = item.primaryText();
            String unit = item.unitLabel();
            if (value.endsWith("%")) { value = value.substring(0, value.length() - 1); unit = "%"; }
            float width = measure(value, 32, true);
            float unitWidth = unit == null ? 0 : measure(unit, 20, false) + 6;
            text(canvas, value, RIGHT - unitWidth, top + 30, 32, INK, true, Paint.Align.RIGHT);
            if (unit != null) text(canvas, unit, RIGHT, top + 30, 20, MUTED, false, Paint.Align.RIGHT);

            float share = item.remaining == null || item.unlimited
                    ? -1 : (float) Math.min(1, Math.max(0, item.remaining / 100));
            bar(canvas, LEFT, top + 50, 300, 6, share);

            String countdown = item.countdown(nowSeconds());
            if (countdown != null) {
                String text = countdown;
                if (item.remaining != null && item.remaining <= 20) text += " · 偏低";
                textFit(canvas, text, LEFT, top + 80, 420, 20, false);
            }
        }
    }

    // ------------------------------------------------------------- empty/state

    private void drawEmpty(Canvas canvas) {
        text(canvas, data.offline ? "未连接 Magpie" : "Magpie 暂无额度", LEFT, 470, 44, INK, true, Paint.Align.LEFT);
        String detail = data.offline ? "确认设备与 Mac 在同一局域网" : "在 Magpie 登录服务商后自动出现";
        textFit(canvas, detail, LEFT, 516, 620, 26, false);
    }

    // ----------------------------------------------------------------- footer

    private void drawFooter(Canvas canvas) {
        float rule = 912;
        filledRect(canvas, LEFT, rule, RIGHT - LEFT, 1, HAIR);

        String left = data.shownAccounts().size() + " 张卡 · " + data.providerCount() + " 服务商";
        textFit(canvas, left, LEFT, rule + 34, 380, 22, false);

        String right = "";
        if (showDeviceBattery && batteryPercent >= 0) {
            right = "电量 " + batteryPercent + "%" + (batteryCharging ? " ↑" : "");
        }
        if (data.offline) {
            right = (right.isEmpty() ? "" : right + "   ") + "上次 " + clock(data.updatedAt);
        }
        if (!right.isEmpty()) textFitRight(canvas, right, RIGHT, rule + 34, 340, 22);

        int pages = data.pageCount();
        if (pages > 1) {
            float width = pages * 18 - 8;
            float start = DESIGN_WIDTH / 2f - width / 2f;
            for (int index = 0; index < pages; index++) {
                float cx = start + index * 18 + 5;
                if (index == data.page) {
                    paint.setStyle(Paint.Style.FILL);
                    paint.setColor(INK);
                    canvas.drawCircle(cx, rule + 62, 5, paint);
                } else {
                    paint.setStyle(Paint.Style.STROKE);
                    paint.setStrokeWidth(2);
                    paint.setColor(HAIR);
                    canvas.drawCircle(cx, rule + 62, 5, paint);
                }
            }
            text(canvas, (data.page + 1) + "/" + pages, RIGHT, rule + 70, 20, MUTED, true, Paint.Align.RIGHT);
        }

        String hint = data.pageCount() <= 1 ? "长按设置"
                : data.page == 0 ? "点击翻页 · 长按设置" : "点击下一屏 · 长按设置";
        text(canvas, hint, DESIGN_WIDTH / 2f, 1004, 20, MUTED, false, Paint.Align.CENTER);
    }

    private static long nowSeconds() { return System.currentTimeMillis() / 1000L; }

    // ------------------------------------------------------------- primitives

    private void outlinedRect(Canvas canvas, float x, float y, float width, float height,
                              float stroke, float radius) {
        paint.setStyle(Paint.Style.STROKE);
        paint.setStrokeWidth(stroke);
        paint.setColor(INK);
        rect.set(x + stroke / 2, y + stroke / 2, x + width - stroke / 2, y + height - stroke / 2);
        canvas.drawRoundRect(rect, radius, radius, paint);
    }

    /** A rounded share bar: light track, ink fill, nothing when the share is unknown. */
    private void bar(Canvas canvas, float x, float y, float width, float height, float share) {
        rect.set(x, y, x + width, y + height);
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(TRACK);
        canvas.drawRoundRect(rect, height / 2, height / 2, paint);
        if (share < 0) return;
        rect.set(x, y, x + Math.max(height, width * share), y + height);
        paint.setColor(INK);
        canvas.drawRoundRect(rect, height / 2, height / 2, paint);
    }

    private void filledRect(Canvas canvas, float x, float y, float width, float height, int color) {
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(color);
        canvas.drawRect(x, y, x + Math.max(0, width), y + height, paint);
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
        while (current > 12 && measure(value, current, bold) > maxWidth) current *= .92f;
        return current;
    }

    private void textFit(Canvas canvas, String value, float x, float y, float maxWidth,
                         float size, boolean bold) {
        text(canvas, value, x, y, fittedSize(value, maxWidth, size, bold), INK, bold, Paint.Align.LEFT);
    }

    private void textFitRight(Canvas canvas, String value, float x, float y, float maxWidth, float size) {
        text(canvas, value, x, y, fittedSize(value, maxWidth, size, true), MUTED, true, Paint.Align.RIGHT);
    }
}
