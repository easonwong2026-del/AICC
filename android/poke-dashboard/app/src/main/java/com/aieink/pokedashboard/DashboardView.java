package com.aieink.pokedashboard;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.DashPathEffect;
import android.graphics.Paint;
import android.graphics.RectF;
import android.graphics.Typeface;
import android.os.Handler;
import android.os.Looper;
import android.view.GestureDetector;
import android.view.MotionEvent;
import android.view.View;

import java.text.SimpleDateFormat;
import java.util.ArrayList;
import java.util.Calendar;
import java.util.Date;
import java.util.List;
import java.util.Locale;

/**
 * High-contrast 16-level grayscale e-ink renderer supporting:
 * 1) Swiss Grid multi-card overview (6 cards/page),
 * 2) Desk Clock + 2x2 bento overview (4 cards/page),
 * 3) Single-account full-width detail view, and
 * 4) Unmistakable offline/cached signaling + touch hit regions.
 */
final class DashboardView extends View {
    interface Listener {
        void onToggleLayoutMode();
        void onManualRefresh();
        void onOpenSettings();
        void onStateChanged();
    }

    interface Clock {
        long nowMillis();
    }

    private enum Action {
        TOGGLE_MODE,
        REFRESH,
        OPEN_SETTINGS,
        PREV_PAGE,
        NEXT_PAGE,
        GO_HOME,
        CLOSE_DETAIL,
        PREV_DETAIL,
        NEXT_DETAIL,
        OPEN_ACCOUNT
    }

    private static final class HitRegion {
        final RectF bounds;
        final Action action;
        final String accountKey;

        HitRegion(float left, float top, float right, float bottom, Action action, String accountKey) {
            this.bounds = new RectF(left, top, right, bottom);
            this.action = action;
            this.accountKey = accountKey;
        }
    }

    private static final Typeface REGULAR = Typeface.DEFAULT;
    private static final Typeface BOLD = Typeface.DEFAULT_BOLD;
    private static final DashPathEffect DASH = new DashPathEffect(new float[]{8f, 6f}, 0f);
    private static final String[] WEEKDAYS = {
            "", "星期日", "星期一", "星期二", "星期三", "星期四", "星期五", "星期六"
    };

    private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final RectF rect = new RectF();
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final Runnable endFlash = () -> { flash = false; invalidate(); };
    private final List<HitRegion> hitRegions = new ArrayList<>();
    private final GestureDetector gestures;

    private Listener listener;
    private Clock clock = System::currentTimeMillis;
    private DashboardData data = new DashboardData();
    private boolean flash;
    private int batteryPercent = -1;
    private boolean batteryCharging;
    private boolean showDeviceBattery = true;
    private float currentScale = 1f;

    DashboardView(Context context) {
        super(context);
        data.offline = true;
        paint.setFontFeatureSettings("tnum");
        setBackgroundColor(EinkTheme.PAPER);
        setClickable(true);
        setLongClickable(true);
        setContentDescription("AICC 额度看板，点击卡片查看详情，点击底部翻页，长按打开设置");
        gestures = new GestureDetector(context, new GestureDetector.SimpleOnGestureListener() {
            @Override public boolean onDown(MotionEvent e) {
                return true;
            }

            @Override public boolean onSingleTapUp(MotionEvent e) {
                handleTap(e.getX(), e.getY());
                return true;
            }

            @Override public void onLongPress(MotionEvent e) {
                if (listener != null) listener.onOpenSettings();
            }
        });
    }

    void setListener(Listener value) {
        listener = value;
    }

    void setClock(Clock value) {
        clock = value == null ? System::currentTimeMillis : value;
    }

    DashboardData getData() {
        return data;
    }

    void setData(DashboardData value) {
        value.clampPage();
        data = value;
        invalidate();
    }

    void nextPage() {
        if (data.detailAccount() != null) {
            data.nextDetailAccount();
        } else if (data.pageCount() > 1) {
            data.nextPage();
        }
        invalidate();
    }

    void prevPage() {
        if (data.detailAccount() != null) {
            data.prevDetailAccount();
        } else if (data.pageCount() > 1) {
            data.prevPage();
        }
        invalidate();
    }

    void setOffline(boolean offline) {
        if (data.offline != offline) {
            data.offline = offline;
            invalidate();
        }
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
        if (!value.equals(data.updatedAt)) {
            data.updatedAt = value;
            invalidate();
        }
    }

    void refreshClock() {
        if (DashboardData.LAYOUT_CLOCK.equals(data.layoutMode) && data.detailAccount() == null) {
            invalidate();
        }
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

    @Override public boolean onTouchEvent(MotionEvent event) {
        return gestures.onTouchEvent(event) || super.onTouchEvent(event);
    }

    private void handleTap(float rawX, float rawY) {
        float lx = rawX / currentScale;
        float ly = rawY / currentScale;
        for (int i = hitRegions.size() - 1; i >= 0; i--) {
            HitRegion region = hitRegions.get(i);
            if (!region.bounds.contains(lx, ly)) continue;
            dispatchAction(region);
            return;
        }
        // Tapping outside explicit controls on overview/clock advances the page.
        if (data.detailAccount() == null && data.pageCount() > 1) {
            data.nextPage();
            invalidate();
            if (listener != null) listener.onStateChanged();
        }
    }

    private void dispatchAction(HitRegion region) {
        switch (region.action) {
            case TOGGLE_MODE:
                if (listener != null) listener.onToggleLayoutMode();
                break;
            case REFRESH:
                if (listener != null) listener.onManualRefresh();
                break;
            case OPEN_SETTINGS:
                if (listener != null) listener.onOpenSettings();
                break;
            case PREV_PAGE:
                if (data.pageCount() > 1) {
                    data.prevPage();
                    invalidate();
                    if (listener != null) listener.onStateChanged();
                }
                break;
            case NEXT_PAGE:
                if (data.pageCount() > 1) {
                    data.nextPage();
                    invalidate();
                    if (listener != null) listener.onStateChanged();
                }
                break;
            case GO_HOME:
                if (data.page != 0 || data.selectedAccountId != null) {
                    data.goHome();
                    invalidate();
                    if (listener != null) listener.onStateChanged();
                }
                break;
            case CLOSE_DETAIL:
                data.closeDetail();
                invalidate();
                if (listener != null) listener.onStateChanged();
                break;
            case PREV_DETAIL:
                data.prevDetailAccount();
                invalidate();
                if (listener != null) listener.onStateChanged();
                break;
            case NEXT_DETAIL:
                data.nextDetailAccount();
                invalidate();
                if (listener != null) listener.onStateChanged();
                break;
            case OPEN_ACCOUNT:
                for (DashboardData.Account account : data.orderedAccounts()) {
                    if (account.stableKey().equals(region.accountKey)) {
                        data.openDetail(account);
                        invalidate();
                        if (listener != null) listener.onStateChanged();
                        break;
                    }
                }
                break;
        }
    }

    @Override protected void onDraw(Canvas canvas) {
        super.onDraw(canvas);
        hitRegions.clear();
        if (flash) {
            canvas.drawColor(EinkTheme.INK);
            return;
        }
        canvas.drawColor(EinkTheme.PAPER);

        int viewW = Math.max(1, getWidth());
        int viewH = Math.max(1, getHeight());
        currentScale = Math.max(0.25f, Math.min(viewW / EinkTheme.DESIGN_WIDTH, viewH / EinkTheme.DESIGN_HEIGHT));
        float w = viewW / currentScale;
        float h = viewH / currentScale;
        float left = 28f;
        float right = w - 28f;

        canvas.save();
        canvas.scale(currentScale, currentScale);

        DashboardData.Account detail = data.detailAccount();
        float footerTop = h - 82f;
        float contentTop;
        if (detail != null) {
            contentTop = drawDetailHeader(canvas, left, right);
        } else {
            boolean clockMode = DashboardData.LAYOUT_CLOCK.equals(data.layoutMode);
            contentTop = drawMainHeader(canvas, left, right, clockMode);
        }

        if (data.offline) {
            contentTop = drawOfflineBanner(canvas, left, right, contentTop + 10f) + 6f;
        }

        List<DashboardData.Account> ordered = data.orderedAccounts();
        if (ordered.isEmpty()) {
            drawEmpty(canvas, left, right, contentTop + 14f, footerTop - 16f);
            drawMainFooter(canvas, left, right, footerTop, h - 14f, false);
        } else if (detail != null) {
            drawDetailBody(canvas, detail, left, right, contentTop + 10f, footerTop - 12f);
            drawDetailFooter(canvas, left, right, footerTop, h - 14f);
        } else if (DashboardData.LAYOUT_CLOCK.equals(data.layoutMode)) {
            drawDeskClockBody(canvas, left, right, contentTop + 8f, footerTop - 12f);
            drawMainFooter(canvas, left, right, footerTop, h - 14f, true);
        } else {
            drawSwissGridBody(canvas, left, right, contentTop + 10f, footerTop - 12f);
            drawMainFooter(canvas, left, right, footerTop, h - 14f, false);
        }

        canvas.restore();
    }

    // ---------------------------------------------------------------- headers

    private float drawMainHeader(Canvas canvas, float left, float right, boolean clockMode) {
        float y = 24f;
        float boxSize = clockMode ? 36f : 40f;
        filledRect(canvas, left, y, boxSize, boxSize, EinkTheme.INK);
        text(canvas, "AI", left + boxSize / 2f, y + boxSize * 0.68f, clockMode ? 17f : 19f,
                EinkTheme.PAPER, true, Paint.Align.CENTER);

        float brandX = left + boxSize + 12f;
        float brandSize = clockMode ? 24f : 28f;
        text(canvas, "AICC", brandX, y + boxSize * 0.74f, brandSize, EinkTheme.INK, true, Paint.Align.LEFT);
        float pillX = brandX + measure("AICC", brandSize, true) + 12f;
        String modeLabel = clockMode ? "桌搭时钟 ⇄" : "瑞士网格 ⇄";
        float pillW = drawOutlinedPill(canvas, pillX, y + (boxSize - 28f) / 2f, modeLabel, 14f);
        hitRegions.add(new HitRegion(pillX - 6f, y - 4f, pillX + pillW + 6f, y + boxSize + 6f,
                Action.TOGGLE_MODE, null));

        drawRightStatus(canvas, right, y, boxSize);

        float ruleY = y + boxSize + 12f;
        filledRect(canvas, left, ruleY, right - left, 2.5f, EinkTheme.INK);
        return ruleY + 2.5f;
    }

    private float drawDetailHeader(Canvas canvas, float left, float right) {
        float y = 24f;
        float btnH = 38f;
        float btnW = drawOutlinedButton(canvas, left, y, "‹ 返回总览", 17f, btnH, true);
        hitRegions.add(new HitRegion(left - 4f, y - 4f, left + btnW + 6f, y + btnH + 4f,
                Action.CLOSE_DETAIL, null));

        drawRightStatus(canvas, right, y, btnH);

        float ruleY = y + btnH + 14f;
        filledRect(canvas, left, ruleY, right - left, 2.5f, EinkTheme.INK);
        return ruleY + 2.5f;
    }

    private void drawRightStatus(Canvas canvas, float right, float topY, float rowH) {
        float x = right;
        float centerY = topY + rowH * 0.68f;
        if (showDeviceBattery && batteryPercent >= 0) {
            String batt = "电量 " + batteryPercent + "%" + (batteryCharging ? " ↑" : "");
            text(canvas, batt, x, centerY, 16f, EinkTheme.MUTED, true, Paint.Align.RIGHT);
            x -= measure(batt, 16f, true) + 14f;
        }

        float hitLeft;
        if (data.offline) {
            String offlineLabel = "○ OFFLINE · 上次 " + clockStr(data.updatedAt);
            float pw = drawInkPillRight(canvas, x, topY + (rowH - 30f) / 2f, offlineLabel, 15f, 30f);
            hitLeft = x - pw - 6f;
        } else {
            String label;
            if (data.orderedAccounts().isEmpty()) {
                label = "暂无额度";
            } else if ("Cached".equals(data.state)) {
                label = "缓存 · " + clockStr(data.updatedAt);
            } else {
                label = "SYNC " + clockStr(data.updatedAt);
            }
            text(canvas, label, x, centerY, 17f, EinkTheme.INK, true, Paint.Align.RIGHT);
            float lw = measure(label, 17f, true);
            float dotX = x - lw - 14f;
            paint.setStyle(Paint.Style.FILL);
            paint.setColor(EinkTheme.INK);
            canvas.drawCircle(dotX, centerY - 6f, 4.5f, paint);
            hitLeft = dotX - 12f;
        }
        hitRegions.add(new HitRegion(hitLeft, topY - 4f, right + 4f, topY + rowH + 6f, Action.REFRESH, null));
    }

    private float drawOfflineBanner(Canvas canvas, float left, float right, float top) {
        float h = 40f;
        rect.set(left, top, right, top + h);
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(EinkTheme.PAPER_ALT);
        canvas.drawRoundRect(rect, 8f, 8f, paint);

        paint.setStyle(Paint.Style.STROKE);
        paint.setStrokeWidth(1.5f);
        paint.setColor(EinkTheme.INK);
        paint.setPathEffect(DASH);
        canvas.drawRoundRect(rect, 8f, 8f, paint);
        paint.setPathEffect(null);

        textFit(canvas, "当前显示离线缓存 · 请确认与 Mac 同局域网", left + 14f, top + 26f,
                (right - left) - 120f, 15f, EinkTheme.INK, true);
        text(canvas, "点击重试", right - 14f, top + 26f, 15f, EinkTheme.INK, true, Paint.Align.RIGHT);
        float rw = measure("点击重试", 15f, true);
        filledRect(canvas, right - 14f - rw, top + 29f, rw, 1.5f, EinkTheme.INK);

        hitRegions.add(new HitRegion(left, top, right, top + h, Action.REFRESH, null));
        return top + h;
    }

    // --------------------------------------------------------- swiss grid UI

    private void drawSwissGridBody(Canvas canvas, float left, float right, float top, float bottom) {
        List<DashboardData.Account> pageRows = data.pageAccounts();
        int totalCount = data.orderedAccounts().size();
        int pages = data.pageCount();
        String sortName = DashboardData.SORT_PROVIDER.equals(data.sortMode) ? "按服务商排序" : "低额优先排序";

        if (data.page == 0) {
            float cursor = top;
            text(canvas, "重点关注 · 剩余最低", left, cursor + 18f, 15f, EinkTheme.MUTED, true, Paint.Align.LEFT);
            String rightMeta = totalCount + " 张卡 / " + data.providerCount() + " 服务商";
            text(canvas, rightMeta, right, cursor + 18f, 15f, EinkTheme.MUTED, true, Paint.Align.RIGHT);
            cursor += 28f;

            float availH = bottom - cursor;
            boolean hasGrid = pageRows.size() > 2;
            float focusH = hasGrid ? Math.min(238f, availH * 0.345f) : Math.min(290f, availH * 0.55f);
            float colGap = 16f;
            float colW = (right - left - colGap) / 2f;

            if (pageRows.size() == 1) {
                drawAccountCard(canvas, pageRows.get(0), left, cursor, right - left, focusH, true);
            } else {
                drawAccountCard(canvas, pageRows.get(0), left, cursor, colW, focusH, true);
                drawAccountCard(canvas, pageRows.get(1), left + colW + colGap, cursor, colW, focusH, true);
            }

            if (!hasGrid) return;

            cursor += focusH + 14f;
            String gridTitle = pages > 1 ? "各服务商代表额度（1 / " + pages + " 页）" : "全部账号额度";
            text(canvas, gridTitle, left, cursor + 18f, 15f, EinkTheme.MUTED, true, Paint.Align.LEFT);
            text(canvas, sortName, right, cursor + 18f, 15f, EinkTheme.MUTED, true, Paint.Align.RIGHT);
            cursor += 28f;

            float rowGap = 14f;
            float cardH = (bottom - cursor - rowGap) / 2f;
            int gridCount = Math.min(4, pageRows.size() - 2);
            for (int i = 0; i < gridCount; i++) {
                int r = i / 2;
                int c = i % 2;
                float cx = left + c * (colW + colGap);
                float cy = cursor + r * (cardH + rowGap);
                drawAccountCard(canvas, pageRows.get(2 + i), cx, cy, colW, cardH, false);
            }
        } else {
            float cursor = top;
            text(canvas, "全部账号（第 " + (data.page + 1) + " / " + pages + " 页）",
                    left, cursor + 18f, 15f, EinkTheme.MUTED, true, Paint.Align.LEFT);
            text(canvas, sortName, right, cursor + 18f, 15f, EinkTheme.MUTED, true, Paint.Align.RIGHT);
            cursor += 28f;

            float colGap = 16f;
            float rowGap = 14f;
            float colW = (right - left - colGap) / 2f;
            float cardH = (bottom - cursor - rowGap * 2f) / 3f;
            int gridCount = Math.min(6, pageRows.size());
            for (int i = 0; i < gridCount; i++) {
                int r = i / 2;
                int c = i % 2;
                float cx = left + c * (colW + colGap);
                float cy = cursor + r * (cardH + rowGap);
                drawAccountCard(canvas, pageRows.get(i), cx, cy, colW, cardH, false);
            }
        }
    }

    // ---------------------------------------------------------- desk clock UI

    private void drawDeskClockBody(Canvas canvas, float left, float right, float top, float bottom) {
        long nowMs = clock.nowMillis();
        Date nowDate = new Date(nowMs);
        String hm = new SimpleDateFormat("HH:mm", Locale.US).format(nowDate);
        Calendar cal = Calendar.getInstance();
        cal.setTime(nowDate);
        String weekday = WEEKDAYS[cal.get(Calendar.DAY_OF_WEEK)];
        String dateLine = new SimpleDateFormat("yyyy 年 M 月 d 日", Locale.CHINA).format(nowDate)
                + " · " + weekday + " · "
                + (data.offline ? "离线缓存中" : data.providerCount() + " 服务商在线");

        float availH = bottom - top;
        boolean compactClock = data.offline || availH < 770f;
        float heroH = compactClock ? 266f : 292f;
        float digitSize = compactClock ? 138f : 156f;

        float centerX = (left + right) / 2f;
        text(canvas, hm, centerX, top + digitSize * 0.88f, digitSize, EinkTheme.INK, true, Paint.Align.CENTER);
        textFitCenter(canvas, dateLine, centerX, top + digitSize * 0.88f + 38f, right - left - 20f, 21f,
                EinkTheme.MUTED, true);

        String strip = buildClockFocusStrip();
        float stripY = top + digitSize * 0.88f + 56f;
        drawCenteredInkPill(canvas, centerX, stripY, strip, right - left - 24f, 15f, 32f);

        float ruleY = top + heroH;
        filledRect(canvas, left, ruleY, right - left, 2f, EinkTheme.INK);

        float cursor = ruleY + 14f;
        text(canvas, "核心额度（点卡片看详情）", left, cursor + 18f, 15f, EinkTheme.MUTED, true, Paint.Align.LEFT);
        text(canvas, data.orderedAccounts().size() + " 张卡 · " + (data.page + 1) + " / " + data.pageCount() + " 页",
                right, cursor + 18f, 15f, EinkTheme.MUTED, true, Paint.Align.RIGHT);
        cursor += 28f;

        List<DashboardData.Account> pageRows = data.pageAccounts();
        float colGap = 16f;
        float rowGap = 14f;
        float colW = (right - left - colGap) / 2f;
        float cardH = (bottom - cursor - rowGap) / 2f;
        int count = Math.min(4, pageRows.size());
        for (int i = 0; i < count; i++) {
            int r = i / 2;
            int c = i % 2;
            float cx = left + c * (colW + colGap);
            float cy = cursor + r * (cardH + rowGap);
            drawAccountCard(canvas, pageRows.get(i), cx, cy, colW, cardH, false);
        }
    }

    private String buildClockFocusStrip() {
        if (data.offline) {
            return "○ 离线缓存模式 · 上次成功 " + clockStr(data.updatedAt);
        }
        List<DashboardData.Account> ordered = data.orderedAccounts();
        if (ordered.isEmpty()) return "● 等待连接 Magpie 额度数据";
        StringBuilder sb = new StringBuilder("● 重点关注  ");
        int added = 0;
        for (int i = 0; i < ordered.size() && added < 2; i++) {
            DashboardData.Account acc = ordered.get(i);
            DashboardData.Item lead = acc.leadItem();
            if (added > 0) sb.append("  ·  ");
            sb.append(acc.providerName).append(" ");
            if (acc.isBalance()) {
                sb.append(acc.balanceText());
            } else if (lead != null) {
                sb.append(lead.primaryText());
            } else {
                sb.append("--");
            }
            added++;
        }
        return sb.toString();
    }

    // -------------------------------------------------------- shared card UI

    private void drawAccountCard(Canvas canvas, DashboardData.Account account,
                                 float x, float y, float w, float h, boolean focus) {
        boolean alert = account.hasLowItem();
        float stroke = alert ? EinkTheme.ALERT_STROKE : EinkTheme.CARD_STROKE;
        outlinedRect(canvas, x, y, w, h, stroke, EinkTheme.CARD_RADIUS, EinkTheme.INK);
        hitRegions.add(new HitRegion(x, y, x + w, y + h, Action.OPEN_ACCOUNT, account.stableKey()));

        float padX = 16f;
        float padTop = 14f;
        float padBot = 12f;
        float innerLeft = x + padX;
        float innerRight = x + w - padX;
        float innerW = innerRight - innerLeft;

        // Top row: badge on right + title/subtitle on left
        String badge = cardBadgeText(account);
        float badgeW = 0f;
        if (badge != null) {
            if (alert) {
                badgeW = drawInkPillRight(canvas, innerRight, y + padTop, badge, 13.5f, 26f) + 8f;
            } else {
                badgeW = drawOutlinedPillRight(canvas, innerRight, y + padTop, badge, 13.5f, 26f) + 8f;
            }
        }

        if (focus) {
            textFit(canvas, account.providerName, innerLeft, y + padTop + 20f,
                    innerW - badgeW, 20f, EinkTheme.INK, true);
            DashboardData.Item lead = account.leadItem();
            String sub = account.label + (lead != null ? " · " + lead.label() : "");
            textFit(canvas, sub, innerLeft, y + padTop + 42f, innerW - 4f, 14.5f, EinkTheme.MUTED, true);
        } else {
            textFit(canvas, account.title(), innerLeft, y + padTop + 20f,
                    innerW - badgeW, 19f, EinkTheme.INK, true);
            String sub = cardSubtitle(account);
            textFit(canvas, sub, innerLeft, y + padTop + 41f, innerW - 4f, 14f, EinkTheme.MUTED, true);
        }

        // Middle row: large tabular value + unit + right chip
        DashboardData.Item lead = account.leadItem();
        String val;
        String unit = null;
        if (account.isBalance()) {
            val = account.balanceText();
            unit = "余额";
        } else if (lead == null) {
            val = account.failed ? "失效" : "—";
        } else {
            val = lead.primaryText();
            unit = lead.unitLabel();
            if (val.endsWith("%")) {
                val = val.substring(0, val.length() - 1);
                unit = "%";
            }
        }

        String chip = cardRightChip(account, lead);
        float chipReserve = chip == null ? 0f : measure(chip, 13.5f, true) + 26f;

        float maxValW = Math.max(80f, innerW - chipReserve - (unit != null ? 38f : 8f));
        float numSize = focus ? (h >= 210f ? 64f : 56f) : (h >= 210f ? 52f : 44f);
        if (account.isBalance()) numSize = Math.min(numSize, 34f);
        while (numSize > 24f && measure(val, numSize, true) > maxValW) {
            numSize *= 0.88f;
        }
        float valW = measure(val, numSize, true);
        float numBaseline = y + h * 0.65f;
        if (focus) numBaseline = Math.min(numBaseline, y + h - 54f);
        else numBaseline = Math.min(numBaseline, y + h - 46f);

        text(canvas, val, innerLeft, numBaseline, numSize, EinkTheme.INK, true, Paint.Align.LEFT);
        if (unit != null && !unit.isEmpty()) {
            float unitSize = focus ? 21f : 18f;
            float unitMaxW = Math.max(20f, innerW - valW - chipReserve - 6f);
            textFit(canvas, unit, innerLeft + valW + 6f, numBaseline, unitMaxW, unitSize, EinkTheme.MUTED, true);
        }
        if (chip != null) {
            drawSoftChipRight(canvas, innerRight, numBaseline - 22f, chip, 13.5f);
        }

        // Bottom: full-width bar + meta row
        float metaBaseline = y + h - padBot;
        float barY = metaBaseline - 28f;
        float share = -1f;
        if (!account.isBalance() && lead != null && lead.remaining != null && !lead.unlimited) {
            share = (float) Math.min(1.0, Math.max(0.0, lead.remaining / 100.0));
        }
        bar(canvas, innerLeft, barY, innerW, focus ? 10f : 8.5f, share);

        String leftMeta = cardLeftMeta(account, lead);
        String rightMeta = cardRightMeta(account, lead);
        float rightMetaW = rightMeta == null ? 0f : measure(rightMeta, 14f, true) + 10f;
        if (leftMeta != null) {
            textFit(canvas, leftMeta, innerLeft, metaBaseline, Math.max(60f, innerW - rightMetaW),
                    14f, EinkTheme.MUTED, true);
        }
        if (rightMeta != null) {
            int rColor = alert ? EinkTheme.INK : EinkTheme.MUTED;
            text(canvas, rightMeta, innerRight, metaBaseline, 14f, rColor, true, Paint.Align.RIGHT);
        }
    }

    private String cardBadgeText(DashboardData.Account account) {
        if (account.failed) return "失败";
        if (account.deferred) return "跳过";
        if (account.hasLowItem()) return "偏低";
        if (account.stale) return "缓存";
        if (account.items.size() > 1) return account.items.size() + " 窗口";
        if (account.plan != null && !account.plan.isEmpty()) return account.plan;
        return null;
    }

    private static String cardSubtitle(DashboardData.Account account) {
        DashboardData.Item lead = account.leadItem();
        if (account.isBalance()) {
            return (account.plan != null ? account.plan + " · " : "") + "余额账户";
        }
        if (lead == null) return account.plan != null ? account.plan : "暂无窗口";
        if (account.plan != null && !account.plan.isEmpty()) {
            return account.plan + " · " + lead.label();
        }
        return lead.label();
    }

    private static String cardRightChip(DashboardData.Account account, DashboardData.Item lead) {
        if (account.isBalance()) return null;
        if (account.items.size() > 1) {
            for (DashboardData.Item item : account.items) {
                if (item != lead && !item.isEstimated()) {
                    return item.label() + " " + item.primaryText();
                }
            }
            return "共 " + account.items.size() + " 窗口 ›";
        }
        return lead == null ? null : lead.totalText();
    }

    private String cardLeftMeta(DashboardData.Account account, DashboardData.Item lead) {
        if (account.isBalance()) return account.statusText() != null ? account.statusText() : "按实际余额计费";
        if (lead == null) return account.statusText() != null ? account.statusText() : "等待刷新";
        String countdown = lead.countdown(nowSeconds());
        if (countdown != null) return countdown;
        return lead.totalText() != null ? lead.totalText() : "实时额度";
    }

    private static String cardRightMeta(DashboardData.Account account, DashboardData.Item lead) {
        if (account.failed) return "需检查";
        if (account.hasLowItem()) return "优先关注";
        if (account.stale) return "缓存";
        if (lead != null && lead.remaining != null && lead.remaining >= 99.5) return "满额";
        return account.items.size() > 1 ? "详情 ›" : "正常";
    }

    // ------------------------------------------------------------- detail UI

    private void drawDetailBody(Canvas canvas, DashboardData.Account account,
                                float left, float right, float top, float bottom) {
        float titleY = top + 34f;
        float planW = 0f;
        if (account.plan != null && !account.plan.isEmpty()) {
            planW = drawOutlinedPillRight(canvas, right, top + 6f, account.plan, 15f, 30f) + 12f;
        }
        textFit(canvas, account.title(), left, titleY, (right - left) - planW, 32f, EinkTheme.INK, true);

        String sub = account.isBalance()
                ? "余额账户 · " + account.balanceText()
                : account.items.size() + " 个额度窗口";
        if (account.statusText() != null) sub = account.statusText() + " · " + sub;
        textFit(canvas, sub, left, titleY + 30f, right - left, 16f, EinkTheme.MUTED, true);

        float headerRuleY = titleY + 46f;
        filledRect(canvas, left, headerRuleY, right - left, 2.5f, EinkTheme.INK);

        float listTop = headerRuleY + 12f;
        if (account.items.isEmpty()) {
            float cardH = Math.min(220f, bottom - listTop);
            outlinedRect(canvas, left, listTop, right - left, cardH, EinkTheme.CARD_STROKE,
                    EinkTheme.CARD_RADIUS, EinkTheme.INK);
            text(canvas, account.isBalance() ? "当前余额" : "暂无额度窗口",
                    left + 22f, listTop + 44f, 20f, EinkTheme.MUTED, true, Paint.Align.LEFT);
            textFit(canvas, account.isBalance() ? account.balanceText() : "等待下一次 Magpie 读取",
                    left + 22f, listTop + 132f, right - left - 44f, 64f, EinkTheme.INK, true);
            return;
        }

        int count = account.items.size();
        float availH = bottom - listTop;
        int slots = Math.max(4, count);
        float pitch = Math.min(144f, availH / slots);
        boolean compact = count > 4;

        for (int i = 0; i < count; i++) {
            DashboardData.Item item = account.items.get(i);
            float rowTop = listTop + i * pitch;
            float rowH = pitch - 8f;
            boolean low = item.isLow();

            if (low) {
                rect.set(left, rowTop + 2f, right, rowTop + rowH);
                paint.setStyle(Paint.Style.FILL);
                paint.setColor(EinkTheme.PAPER_ALT);
                canvas.drawRoundRect(rect, 10f, 10f, paint);
                outlinedRect(canvas, left, rowTop + 2f, right - left, rowH - 2f, 2.5f, 10f, EinkTheme.INK);
            } else if (i > 0) {
                filledRect(canvas, left, rowTop, right - left, 1.5f, EinkTheme.HAIR);
            }

            float innerL = low ? left + 14f : left;
            float innerR = low ? right - 14f : right;
            float innerW = innerR - innerL;

            // Value & unit on right
            String val = item.primaryText();
            String unit = item.unitLabel();
            if (val.endsWith("%")) {
                val = val.substring(0, val.length() - 1);
                unit = "%";
            }
            String total = item.totalText();
            String rightExtra = "";
            if (unit != null && !unit.isEmpty()) rightExtra += unit;
            if (total != null) rightExtra += (rightExtra.isEmpty() ? "" : " · ") + total;

            float valSize = compact ? 28f : 34f;
            float extraSize = compact ? 15f : 17f;
            float extraW = rightExtra.isEmpty() ? 0f : measure(rightExtra, extraSize, true) + 6f;
            float valW = measure(val, valSize, true);
            float topBaseline = rowTop + (compact ? 32f : 40f);

            if (!rightExtra.isEmpty()) {
                text(canvas, rightExtra, innerR, topBaseline, extraSize, EinkTheme.MUTED, true, Paint.Align.RIGHT);
            }
            text(canvas, val, innerR - extraW, topBaseline, valSize, EinkTheme.INK, true, Paint.Align.RIGHT);

            // Label + low pill on left
            float nameMaxW = innerW - valW - extraW - 16f;
            String label = item.detailLabel();
            if (low) {
                float labelW = Math.min(nameMaxW - 64f, measure(label, compact ? 20f : 23f, true));
                textFit(canvas, label, innerL, topBaseline, Math.max(80f, labelW),
                        compact ? 20f : 23f, EinkTheme.INK, true);
                drawInkPillLeft(canvas, innerL + Math.max(80f, labelW) + 10f, topBaseline - 22f, "偏低", 13.5f, 24f);
            } else {
                textFit(canvas, label, innerL, topBaseline, Math.max(100f, nameMaxW),
                        compact ? 20f : 23f, EinkTheme.INK, true);
            }

            // Full-width progress bar
            float barY = topBaseline + (compact ? 14f : 18f);
            float share = (item.remaining == null || item.unlimited)
                    ? -1f : (float) Math.min(1.0, Math.max(0.0, item.remaining / 100.0));
            bar(canvas, innerL, barY, innerW, compact ? 8f : 10f, share, low);

            // Bottom meta row
            float metaY = barY + (compact ? 28f : 34f);
            String countdown = item.countdown(nowSeconds());
            String leftNote = countdown != null ? countdown : "有效期以官方规则为准";
            textFit(canvas, leftNote, innerL, metaY, innerW * 0.65f, compact ? 14f : 15.5f,
                    low ? EinkTheme.INK : EinkTheme.MUTED, true);

            String rightNote;
            if (item.unlimited) rightNote = "无上限";
            else if (item.remaining != null) rightNote = Math.round(item.remaining) + "% 可用";
            else rightNote = "未知";
            if (low) rightNote = "低额告警";
            text(canvas, rightNote, innerR, metaY, compact ? 14f : 15.5f,
                    low ? EinkTheme.INK : EinkTheme.MUTED, true, Paint.Align.RIGHT);
        }
    }

    // -------------------------------------------------------------- empty UI

    private void drawEmpty(Canvas canvas, float left, float right, float top, float bottom) {
        float h = Math.min(340f, bottom - top);
        float y = top + Math.max(0f, (bottom - top - h) * 0.35f);
        outlinedRect(canvas, left, y, right - left, h, 2f, 14f, EinkTheme.INK);

        String title = data.offline ? "未连接 Mac AICC" : "Magpie 暂无额度";
        String sub = data.offline
                ? "请确认设备与 Mac 在同一局域网，并在 Mac 设置中开启「允许局域网设备连接」"
                : "请在 Mac 端 Magpie 登录服务商，或调整设置中的服务商筛选";
        text(canvas, title, left + 28f, y + 76f, 36f, EinkTheme.INK, true, Paint.Align.LEFT);
        textFit(canvas, sub, left + 28f, y + 124f, right - left - 56f, 18f, EinkTheme.MUTED, true);

        float btnY = y + h - 76f;
        float btnW = 152f;
        drawOutlinedButton(canvas, left + 28f, btnY, "打开设置", 17f, 46f, false, btnW);
        hitRegions.add(new HitRegion(left + 28f, btnY, left + 28f + btnW, btnY + 46f,
                Action.OPEN_SETTINGS, null));

        drawInkButton(canvas, left + 28f + btnW + 16f, btnY, "立即重试", 17f, 46f, btnW);
        hitRegions.add(new HitRegion(left + 28f + btnW + 16f, btnY, left + 28f + btnW * 2f + 16f, btnY + 46f,
                Action.REFRESH, null));
    }

    // --------------------------------------------------------------- footers

    private void drawMainFooter(Canvas canvas, float left, float right, float top, float bottom, boolean clockMode) {
        filledRect(canvas, left, top, right - left, 2f, EinkTheme.INK);
        float btnH = 42f;
        float btnY = top + (bottom - top - btnH) / 2f;

        if (clockMode) {
            float switchW = drawOutlinedButton(canvas, left, btnY, "切换网格", 16f, btnH, true);
            hitRegions.add(new HitRegion(left, btnY - 4f, left + switchW, btnY + btnH + 4f,
                    Action.TOGGLE_MODE, null));
        } else {
            boolean canPage = data.pageCount() > 1;
            float prevW = drawOutlinedButton(canvas, left, btnY, "‹ 上一页", 16f, btnH, canPage);
            hitRegions.add(new HitRegion(left, btnY - 4f, left + prevW, btnY + btnH + 4f,
                    Action.PREV_PAGE, null));
        }

        float centerX = (left + right) / 2f;
        String pageLabel = (data.page + 1) + " / " + data.pageCount() + " 页";
        text(canvas, pageLabel, centerX, btnY + 21f, 17.5f, EinkTheme.INK, true, Paint.Align.CENTER);
        String hint = clockMode ? "长按任意处打开设置" : "点卡片看详情 · 点右上角刷新";
        text(canvas, hint, centerX, btnY + 39f, 12.5f, EinkTheme.MUTED, true, Paint.Align.CENTER);
        hitRegions.add(new HitRegion(centerX - 110f, top, centerX + 110f, bottom, Action.GO_HOME, null));

        float gearW = drawOutlinedButton(canvas, right - 46f, btnY, "⚙", 18f, btnH, true, 46f);
        hitRegions.add(new HitRegion(right - gearW, btnY - 4f, right, btnY + btnH + 4f,
                Action.OPEN_SETTINGS, null));

        float curRight = right - gearW - 10f;
        if (clockMode) {
            float refW = 46f;
            drawOutlinedButton(canvas, curRight - refW, btnY, "⟳", 18f, btnH, true, refW);
            hitRegions.add(new HitRegion(curRight - refW, btnY - 4f, curRight, btnY + btnH + 4f,
                    Action.REFRESH, null));
            curRight -= refW + 10f;
        }
        boolean canPage = data.pageCount() > 1;
        float nextW = 104f;
        drawOutlinedButton(canvas, curRight - nextW, btnY, "下一页 ›", 16f, btnH, canPage, nextW);
        hitRegions.add(new HitRegion(curRight - nextW, btnY - 4f, curRight, btnY + btnH + 4f,
                Action.NEXT_PAGE, null));
    }

    private void drawDetailFooter(Canvas canvas, float left, float right, float top, float bottom) {
        filledRect(canvas, left, top, right - left, 2f, EinkTheme.INK);
        float btnH = 42f;
        float btnY = top + (bottom - top - btnH) / 2f;
        int total = data.orderedAccounts().size();
        int idx = data.detailAccountIndex() + 1;
        boolean canSwitch = total > 1;

        float prevW = drawOutlinedButton(canvas, left, btnY, "‹ 上一账号", 16f, btnH, canSwitch);
        hitRegions.add(new HitRegion(left, btnY - 4f, left + prevW, btnY + btnH + 4f,
                Action.PREV_DETAIL, null));

        float centerX = (left + right) / 2f;
        text(canvas, "详情 · " + idx + " / " + total, centerX, btnY + 21f, 17.5f,
                EinkTheme.INK, true, Paint.Align.CENTER);
        String sub = data.offline ? "离线缓存快照 · " + clockStr(data.updatedAt) : "点击这里返回总览";
        text(canvas, sub, centerX, btnY + 39f, 12.5f, EinkTheme.MUTED, true, Paint.Align.CENTER);
        hitRegions.add(new HitRegion(centerX - 110f, top, centerX + 110f, bottom,
                Action.CLOSE_DETAIL, null));

        float nextW = 122f;
        drawOutlinedButton(canvas, right - nextW, btnY, "下一账号 ›", 16f, btnH, canSwitch, nextW);
        hitRegions.add(new HitRegion(right - nextW, btnY - 4f, right, btnY + btnH + 4f,
                Action.NEXT_DETAIL, null));
    }

    // ------------------------------------------------------------ primitives

    private long nowSeconds() {
        return clock.nowMillis() / 1000L;
    }

    private static String clockStr(String stamp) {
        String value = stamp == null ? "" : stamp.trim();
        if (value.length() >= 5) {
            String tail = value.substring(value.length() - 5);
            if (tail.matches("\\d{2}:\\d{2}")) return tail;
        }
        return value.isEmpty() ? "--" : value;
    }

    private void outlinedRect(Canvas canvas, float x, float y, float width, float height,
                              float stroke, float radius, int color) {
        paint.setStyle(Paint.Style.STROKE);
        paint.setStrokeWidth(stroke);
        paint.setColor(color);
        rect.set(x + stroke / 2f, y + stroke / 2f, x + width - stroke / 2f, y + height - stroke / 2f);
        canvas.drawRoundRect(rect, radius, radius, paint);
    }

    private void bar(Canvas canvas, float x, float y, float width, float height, float share) {
        bar(canvas, x, y, width, height, share, false);
    }

    private void bar(Canvas canvas, float x, float y, float width, float height, float share, boolean darkTrack) {
        rect.set(x, y, x + width, y + height);
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(darkTrack ? EinkTheme.TRACK_DARK : EinkTheme.TRACK);
        canvas.drawRoundRect(rect, height / 2f, height / 2f, paint);
        if (share < 0) return;
        float filled = Math.max(height, width * share);
        if (share == 0f) filled = 0f;
        if (filled > 0f) {
            rect.set(x, y, x + filled, y + height);
            paint.setColor(EinkTheme.INK);
            canvas.drawRoundRect(rect, height / 2f, height / 2f, paint);
        }
    }

    private void filledRect(Canvas canvas, float x, float y, float width, float height, int color) {
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(color);
        canvas.drawRect(x, y, x + Math.max(0f, width), y + height, paint);
    }

    private float drawOutlinedPill(Canvas canvas, float x, float y, String label, float textSize) {
        float w = measure(label, textSize, true) + 20f;
        float h = 28f;
        outlinedRect(canvas, x, y, w, h, 1.5f, h / 2f, EinkTheme.INK);
        text(canvas, label, x + w / 2f, y + h * 0.68f, textSize, EinkTheme.INK, true, Paint.Align.CENTER);
        return w;
    }

    private float drawOutlinedPillRight(Canvas canvas, float right, float y, String label, float textSize, float h) {
        float w = measure(label, textSize, true) + 20f;
        outlinedRect(canvas, right - w, y, w, h, 1.5f, h / 2f, EinkTheme.INK);
        text(canvas, label, right - w / 2f, y + h * 0.68f, textSize, EinkTheme.INK, true, Paint.Align.CENTER);
        return w;
    }

    private float drawInkPillRight(Canvas canvas, float right, float y, String label, float textSize, float h) {
        float w = measure(label, textSize, true) + 20f;
        rect.set(right - w, y, right, y + h);
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(EinkTheme.INK);
        canvas.drawRoundRect(rect, h / 2f, h / 2f, paint);
        text(canvas, label, right - w / 2f, y + h * 0.68f, textSize, EinkTheme.PAPER, true, Paint.Align.CENTER);
        return w;
    }

    private float drawInkPillLeft(Canvas canvas, float left, float y, String label, float textSize, float h) {
        float w = measure(label, textSize, true) + 18f;
        rect.set(left, y, left + w, y + h);
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(EinkTheme.INK);
        canvas.drawRoundRect(rect, 6f, 6f, paint);
        text(canvas, label, left + w / 2f, y + h * 0.69f, textSize, EinkTheme.PAPER, true, Paint.Align.CENTER);
        return w;
    }

    private void drawCenteredInkPill(Canvas canvas, float centerX, float y, String label,
                                     float maxW, float textSize, float h) {
        String fitted = ellipsize(label, maxW - 32f, textSize, true);
        float w = measure(fitted, textSize, true) + 32f;
        rect.set(centerX - w / 2f, y, centerX + w / 2f, y + h);
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(EinkTheme.INK);
        canvas.drawRoundRect(rect, h / 2f, h / 2f, paint);
        text(canvas, fitted, centerX, y + h * 0.68f, textSize, EinkTheme.PAPER, true, Paint.Align.CENTER);
    }

    private void drawSoftChipRight(Canvas canvas, float right, float y, String label, float textSize) {
        String fitted = ellipsize(label, 140f, textSize, true);
        float w = measure(fitted, textSize, true) + 16f;
        float h = 24f;
        rect.set(right - w, y, right, y + h);
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(EinkTheme.CHIP_BG);
        canvas.drawRoundRect(rect, 6f, 6f, paint);
        text(canvas, fitted, right - w / 2f, y + h * 0.70f, textSize, EinkTheme.INK, true, Paint.Align.CENTER);
    }

    private float drawOutlinedButton(Canvas canvas, float x, float y, String label, float textSize,
                                     float h, boolean enabled) {
        float w = measure(label, textSize, true) + 28f;
        return drawOutlinedButton(canvas, x, y, label, textSize, h, enabled, w);
    }

    private float drawOutlinedButton(Canvas canvas, float x, float y, String label, float textSize,
                                     float h, boolean enabled, float width) {
        int strokeColor = enabled ? EinkTheme.INK : EinkTheme.HAIR;
        int textColor = enabled ? EinkTheme.INK : EinkTheme.DISABLED;
        outlinedRect(canvas, x, y, width, h, 1.6f, 8f, strokeColor);
        text(canvas, label, x + width / 2f, y + h * 0.65f, textSize, textColor, true, Paint.Align.CENTER);
        return width;
    }

    private void drawInkButton(Canvas canvas, float x, float y, String label, float textSize,
                               float h, float width) {
        rect.set(x, y, x + width, y + h);
        paint.setStyle(Paint.Style.FILL);
        paint.setColor(EinkTheme.INK);
        canvas.drawRoundRect(rect, 8f, 8f, paint);
        text(canvas, label, x + width / 2f, y + h * 0.65f, textSize, EinkTheme.PAPER, true, Paint.Align.CENTER);
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

    private String ellipsize(String value, float maxWidth, float size, boolean bold) {
        String raw = value == null || value.isEmpty() ? "--" : value;
        if (measure(raw, size, bold) <= maxWidth) return raw;
        float ellW = measure("…", size, bold);
        int end = raw.length();
        while (end > 1 && measure(raw.substring(0, end), size, bold) + ellW > maxWidth) {
            end--;
        }
        return raw.substring(0, Math.max(1, end)) + "…";
    }

    private void textFit(Canvas canvas, String value, float x, float y, float maxWidth,
                         float size, int color, boolean bold) {
        String raw = value == null || value.isEmpty() ? "--" : value;
        float current = size;
        float minSize = Math.max(12f, size * 0.78f);
        while (current > minSize && measure(raw, current, bold) > maxWidth) {
            current *= 0.92f;
        }
        String display = measure(raw, current, bold) > maxWidth ? ellipsize(raw, maxWidth, current, bold) : raw;
        text(canvas, display, x, y, current, color, bold, Paint.Align.LEFT);
    }

    private void textFitCenter(Canvas canvas, String value, float cx, float y, float maxWidth,
                               float size, int color, boolean bold) {
        String raw = value == null || value.isEmpty() ? "--" : value;
        float current = size;
        float minSize = Math.max(12f, size * 0.78f);
        while (current > minSize && measure(raw, current, bold) > maxWidth) {
            current *= 0.92f;
        }
        String display = measure(raw, current, bold) > maxWidth ? ellipsize(raw, maxWidth, current, bold) : raw;
        text(canvas, display, cx, y, current, color, bold, Paint.Align.CENTER);
    }
}
