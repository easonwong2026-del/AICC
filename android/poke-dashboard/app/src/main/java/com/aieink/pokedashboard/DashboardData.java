package com.aieink.pokedashboard;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.List;
import java.util.Locale;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * The bridge's generic quota snapshot (schema 3): every provider Magpie
 * reports, every account, every window.  The same rules live in
 * collectors/magpie.py and QuotaModel.swift; change the three together.
 */
final class DashboardData {
    static final int SCHEMA_VERSION = 3;
    static final int CARDS_PER_PAGE = 4;

    static final class Item {
        String name = "";
        String display;
        String remainingText;
        String reset;
        String unit;
        String pool;
        /** Set when two windows of one account share a name; disambiguates them. */
        String qualifier;
        Double remaining;
        Double amount;
        Double limit;
        /** Epoch seconds Magpie reported for the next reset, when it did. */
        Double resetAt;
        boolean unlimited;

        boolean isPercent() {
            return !unlimited && display == null && remaining != null;
        }

        String primaryText() {
            if (unlimited) return "∞";
            if (remainingText != null && !remainingText.isEmpty()) return remainingText;
            if (display != null && !display.isEmpty()) return display;
            if (remaining != null) return Math.round(remaining) + "%";
            return "—";
        }

        /** "7 days" as a person reads it: "7 天". Vendor names are kept as-is. */
        String windowLabel() {
            String key = name.trim().toLowerCase(Locale.ROOT);
            Matcher match = WINDOW.matcher(key);
            if (match.find()) {
                String count = match.group(1);
                String unit = match.group(2);
                if (unit.startsWith("day")) return count + " 天";
                if (unit.startsWith("hour")) return count + " 小时";
                if (unit.startsWith("week")) return count + " 周";
                if (unit.startsWith("month")) return count + " 月";
                return count + " 分";
            }
            if (key.startsWith("credit")) return "积分";
            if (key.startsWith("prompt")) return "次";
            return name;
        }

        /** The label to draw: the window name, plus its pool when names repeat. */
        String label() {
            return qualifier == null ? windowLabel() : windowLabel() + " · " + qualifier;
        }

        /** "3 天 4 时后重置"; falls back to the vendor's own text when no epoch. */
        String countdown(long nowSeconds) {
            if (resetAt == null) return reset == null || reset.isEmpty() ? null : reset;
            long seconds = Math.round(resetAt) - nowSeconds;
            if (seconds <= 0) return "即将重置";
            long minutes = seconds / 60;
            if (minutes < 60) return Math.max(1, minutes) + " 分后重置";
            long hours = minutes / 60;
            if (hours < 24) return hours + " 时 " + (minutes % 60) + " 分后重置";
            return (hours / 24) + " 天 " + (hours % 24) + " 时后重置";
        }

        /** "共 4727" for a counted window, null when there is nothing to total. */
        String totalText() {
            if (unlimited || limit == null || limit <= 0 || unit == null) return null;
            return "共 " + Math.round(limit);
        }

        /** Magpie names units in English; the panel is Chinese, like the widget. */
        String unitLabel() {
            if (unit == null || unit.isEmpty()) return null;
            String key = unit.trim().toLowerCase(java.util.Locale.ROOT);
            if (key.startsWith("credit")) return "积分";
            if (key.startsWith("prompt") || key.startsWith("request") || key.startsWith("call")) return "次";
            return unit;
        }
    }

    static final class Account {
        String id = "";
        String provider = "";
        String providerName = "";
        String label = "";
        String kind = "subscription";
        String plan;
        String balance;
        final List<Item> items = new ArrayList<>();
        boolean stale;
        boolean failed;
        boolean deferred;
        Double readAt;

        String title() {
            return providerName + " · " + label;
        }

        boolean isBalance() {
            return "balance".equals(kind);
        }

        String balanceText() {
            return balance == null || balance.isEmpty() ? "—" : balance;
        }

        Item primary() {
            return items.isEmpty() ? null : items.get(0);
        }

        Item secondary() {
            return items.size() > 1 ? items.get(1) : null;
        }

        String statusText() {
            if (failed) return "读取失败";
            if (deferred) return "本轮跳过";
            if (stale) return "缓存";
            return null;
        }
    }

    private static final Pattern WINDOW = Pattern.compile(
            "^(\\d+)\\s*(day|days|hour|hours|week|weeks|month|months|minute|minutes|min|mins)");

    /** One account together with the window the panel leads with. */
    static final class Slot {
        final Account account;
        final Item item;

        Slot(Account account, Item item) {
            this.account = account;
            this.item = item;
        }
    }

    final List<Account> accounts = new ArrayList<>();
    String renderKey = "";
    String updatedAt = "--";
    String state = "Connecting";
    boolean available;
    boolean offline;
    /** Cards wanted on screen: 1, 2, or every account (paged four at a time). */
    int displayLimit = CARDS_PER_PAGE;
    int page;

    static DashboardData parse(String raw) throws Exception {
        return parse(raw, "all");
    }

    static DashboardData parse(String raw, String displayMode) throws Exception {
        JSONObject root = new JSONObject(raw);
        DashboardData data = new DashboardData();
        data.displayLimit = limitFor(displayMode);
        data.offline = false;
        data.updatedAt = root.optString("updated_at", "--");
        data.state = root.optString("state", "Connected");
        data.available = root.optBoolean("available", false);

        JSONArray pool = root.optJSONArray("accounts");
        if (root.optInt("schema_version", 0) >= SCHEMA_VERSION && pool != null) {
            for (int index = 0; index < pool.length(); index++) {
                JSONObject row = pool.optJSONObject(index);
                Account account = account(row);
                if (account != null) data.accounts.add(account);
            }
        } else {
            // An older bridge answers with the Codex-only shape; read it once,
            // explicitly, rather than guessing between two formats per field.
            legacy(root, data);
        }
        if (data.updatedAt == null || data.updatedAt.isEmpty() || "--".equals(data.updatedAt)) {
            data.updatedAt = "--";
        }
        data.renderKey = key(data);
        return data;
    }

    private static int limitFor(String mode) {
        if ("one".equals(mode)) return 1;
        if ("two".equals(mode)) return 2;
        // Every account: shown in pages of four, not silently cut to four.
        return Integer.MAX_VALUE;
    }

    private static Account account(JSONObject row) {
        if (row == null) return null;
        String provider = row.optString("provider", "");
        String kind = row.optString("kind", "");
        if (provider.isEmpty()) return null;
        if (!"subscription".equals(kind) && !"plan".equals(kind) && !"balance".equals(kind)) return null;
        Account account = new Account();
        account.id = row.optString("id", "");
        account.provider = provider;
        account.providerName = row.optString("provider_name", providerName(provider));
        account.label = row.optString("account_label", "***");
        account.kind = kind;
        account.plan = optional(row, "plan");
        account.balance = optional(row, "balance");
        account.stale = row.optBoolean("stale", true);
        account.failed = row.optBoolean("failed", false);
        account.deferred = row.optBoolean("deferred", false);
        if (!row.isNull("read_at")) account.readAt = number(row.opt("read_at"));
        JSONArray items = row.optJSONArray("items");
        for (int index = 0; items != null && index < items.length(); index++) {
            Item item = item(items.optJSONObject(index));
            if (item != null) account.items.add(item);
        }
        // "7 天" appears once per pool on a pooled account; tell them apart.
        for (Item item : account.items) {
            int same = 0;
            for (Item other : account.items) {
                if (other.windowLabel().equals(item.windowLabel())) same++;
            }
            if (same > 1 && item.pool != null && !item.pool.isEmpty()) item.qualifier = item.pool;
        }
        if (account.balance == null && "balance".equals(kind)) {
            JSONArray parts = row.optJSONArray("balance_parts");
            StringBuilder joined = new StringBuilder();
            for (int index = 0; parts != null && index < parts.length(); index++) {
                JSONObject part = parts.optJSONObject(index);
                if (part == null) continue;
                String text = part.optString("text", "");
                if (text.isEmpty()) continue;
                if (joined.length() > 0) joined.append(" · ");
                joined.append(text);
            }
            if (joined.length() > 0) account.balance = joined.toString();
        }
        return account;
    }

    private static Item item(JSONObject row) {
        if (row == null) return null;
        String name = row.optString("name", "");
        if (name.isEmpty()) return null;
        Item item = new Item();
        item.name = name;
        item.unlimited = row.optBoolean("unlimited", false);
        item.remaining = percent(row.opt("remaining"));
        Double used = percent(row.opt("used"));
        if (!item.unlimited && item.remaining == null && used != null) item.remaining = percent(100 - used);
        item.display = optional(row, "display");
        item.remainingText = optional(row, "remaining_text");
        item.reset = optional(row, "reset");
        item.unit = optional(row, "unit");
        item.pool = optional(row, "pool");
        item.resetAt = number(row.opt("resets_at"));
        item.amount = number(row.opt("amount"));
        item.limit = number(row.opt("limit"));
        return item;
    }

    /** The pre-3.0 Codex-only payload, mapped onto the generic model. */
    private static void legacy(JSONObject root, DashboardData data) {
        JSONObject codex = root.optJSONObject("codex");
        if (codex == null) return;
        data.state = codex.optBoolean("available", false)
                ? (codex.optBoolean("stale", false) ? "Cached" : "Connected")
                : codex.optString("state", "Empty");
        data.available = codex.optBoolean("available", false);
        String value = codex.optString("updated_at", "");
        if (!value.isEmpty()) data.updatedAt = value;
        JSONArray pool = codex.optJSONArray("accounts");
        if (pool != null) {
            for (int index = 0; index < pool.length(); index++) {
                JSONObject row = pool.optJSONObject(index);
                if (row == null) continue;
                data.accounts.add(legacyAccount(row, codex.optBoolean("stale", false)));
            }
        } else if (codex.has("weekly") || codex.has("five_hour")) {
            data.accounts.add(legacyAccount(codex, codex.optBoolean("stale", false)));
        }
    }

    private static Account legacyAccount(JSONObject row, boolean poolStale) {
        Account account = new Account();
        account.id = row.optString("id", "");
        account.provider = "codex";
        account.providerName = "Codex";
        account.label = row.optString("label", "Codex");
        account.kind = "subscription";
        account.plan = optional(row, "plan");
        account.stale = poolStale || row.optBoolean("stale", false);
        account.failed = row.optBoolean("needs_reauth", false);
        // The old panel led with the weekly window; keep that reading order.
        addLegacyWindow(account, row.optJSONObject("weekly"), "7 days");
        addLegacyWindow(account, row.optJSONObject("five_hour"), "5 hours");
        return account;
    }

    private static void addLegacyWindow(Account account, JSONObject window, String name) {
        if (window == null) return;
        Item item = new Item();
        item.name = name;
        item.remaining = percent(window.opt("remaining"));
        item.reset = optional(window, "reset");
        account.items.add(item);
    }

    /** Title case for a provider id the bridge didn't name, never a drop. */
    static String providerName(String provider) {
        String name = provider == null ? "" : provider.trim();
        if (name.isEmpty()) return "Unknown";
        StringBuilder named = new StringBuilder();
        for (String word : name.replace('_', '-').split("-")) {
            if (word.isEmpty()) continue;
            if (named.length() > 0) named.append(' ');
            named.append(Character.toUpperCase(word.charAt(0))).append(word.substring(1));
        }
        return named.length() == 0 ? "Unknown" : named.toString();
    }

    private static String optional(JSONObject row, String key) {
        if (row == null || row.isNull(key)) return null;
        String value = row.optString(key, "");
        return value.isEmpty() ? null : value;
    }

    private static Double number(Object value) {
        if (!(value instanceof Number) || value instanceof Boolean) return null;
        double parsed = ((Number) value).doubleValue();
        return Double.isFinite(parsed) ? parsed : null;
    }

    private static Double percent(Object value) {
        Double parsed = number(value);
        if (parsed == null || parsed < 0 || parsed > 100) return null;
        return Math.round(parsed * 100) / 100.0;
    }

    /** Everything that should repaint the panel, and nothing that should not. */
    private static String key(DashboardData data) {
        StringBuilder key = new StringBuilder(data.state).append('|').append(data.available ? 1 : 0)
                .append('|').append(data.displayLimit).append('|').append(data.page);
        for (Account account : data.accounts) {
            key.append('|').append(account.provider).append('/').append(account.label)
                    .append('/').append(account.kind).append('/').append(account.stale ? 1 : 0)
                    .append(account.failed ? 1 : 0).append(account.deferred ? 1 : 0)
                    .append('/').append(account.plan).append('/').append(account.balance);
            for (Item item : account.items) {
                key.append('[').append(item.name).append('=').append(item.primaryText())
                        .append('@').append(item.reset).append('#').append(item.resetAt).append(']');
            }
        }
        return key.toString();
    }

    List<Account> shownAccounts() {
        if (displayLimit >= accounts.size()) return new ArrayList<>(accounts);
        return new ArrayList<>(accounts.subList(0, Math.max(0, displayLimit)));
    }

    /**
     * Accounts that need a page of their own: the ones with more windows than the
     * overview can show.  An account with one or two windows is already told in
     * full by its summary row (and by the hero caption), so an extra page would
     * only be a near-empty screen.
     */
    List<Account> detailAccounts() {
        List<Account> detail = new ArrayList<>();
        for (Account account : shownAccounts()) {
            if (account.items.size() >= 3) detail.add(account);
        }
        return detail;
    }

    /** Page 0 is the overview; each account with three or more windows follows. */
    int pageCount() {
        return 1 + detailAccounts().size();
    }

    void clampPage() {
        page = Math.max(0, Math.min(page, pageCount() - 1));
    }

    /** The account a detail page is about, or null on the overview. */
    Account detailAccount() {
        List<Account> detail = detailAccounts();
        int index = page - 1;
        return index >= 0 && index < detail.size() ? detail.get(index) : null;
    }

    /**
     * The window the overview leads with: whatever is closest to running out.
     * Ties go to the window that resets first, so a stale-but-full account never
     * takes the hero slot from a nearly empty one.
     */
    Slot hero() {
        Slot best = null;
        for (Account account : shownAccounts()) {
            for (Item item : account.items) {
                if (item.remaining == null || item.unlimited) continue;
                if (best == null) { best = new Slot(account, item); continue; }
                double left = item.remaining;
                double current = best.item.remaining;
                if (left < current) { best = new Slot(account, item); continue; }
                if (left == current && resetTime(item) < resetTime(best.item)) {
                    best = new Slot(account, item);
                }
            }
        }
        if (best != null) return best;
        for (Account account : shownAccounts()) {
            if (!account.items.isEmpty()) return new Slot(account, account.items.get(0));
        }
        return null;
    }

    private static double resetTime(Item item) {
        return item.resetAt == null ? Double.MAX_VALUE : item.resetAt;
    }

    /** Up to three accounts under the hero, never the hero's own account. */
    List<Account> summaryAccounts() {
        Slot hero = hero();
        List<Account> rows = new ArrayList<>();
        for (Account account : shownAccounts()) {
            if (hero != null && account == hero.account) continue;
            rows.add(account);
        }
        return rows;
    }

    int hiddenAccountCount() {
        return Math.max(0, summaryAccounts().size() - DashboardView.SUMMARY_ROWS);
    }

    int providerCount() {
        List<String> seen = new ArrayList<>();
        for (Account account : shownAccounts()) {
            if (!seen.contains(account.provider)) seen.add(account.provider);
        }
        return seen.size();
    }

    void nextPage() {
        page = (page + 1) % pageCount();
    }
}
