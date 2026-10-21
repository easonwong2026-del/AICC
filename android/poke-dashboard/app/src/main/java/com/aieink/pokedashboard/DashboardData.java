package com.aieink.pokedashboard;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.List;

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
        Double remaining;
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
                .append('|').append(data.displayLimit);
        for (Account account : data.accounts) {
            key.append('|').append(account.provider).append('/').append(account.label)
                    .append('/').append(account.kind).append('/').append(account.stale ? 1 : 0)
                    .append(account.failed ? 1 : 0).append(account.deferred ? 1 : 0)
                    .append('/').append(account.plan).append('/').append(account.balance);
            for (Item item : account.items) {
                key.append('[').append(item.name).append('=').append(item.primaryText())
                        .append('@').append(item.reset).append(']');
            }
        }
        return key.toString();
    }

    List<Account> shownAccounts() {
        if (displayLimit >= accounts.size()) return new ArrayList<>(accounts);
        return new ArrayList<>(accounts.subList(0, Math.max(0, displayLimit)));
    }

    int pageCount() {
        List<Account> shown = shownAccounts();
        return Math.max(1, (shown.size() + CARDS_PER_PAGE - 1) / CARDS_PER_PAGE);
    }

    void clampPage() {
        page = Math.max(0, Math.min(page, pageCount() - 1));
    }

    List<Account> visibleAccounts() {
        List<Account> shown = shownAccounts();
        int start = page * CARDS_PER_PAGE;
        if (start >= shown.size()) return new ArrayList<>();
        return new ArrayList<>(shown.subList(start, Math.min(shown.size(), start + CARDS_PER_PAGE)));
    }

    void nextPage() {
        if (pageCount() > 1) page = (page + 1) % pageCount();
    }
}
