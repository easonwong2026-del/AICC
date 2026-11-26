package com.aieink.pokedashboard;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * The bridge's generic quota snapshot (schema 3): every provider Magpie
 * reports, every account, every window.  The same rules live in
 * collectors/magpie.py and QuotaModel.swift; change the three together.
 */
final class DashboardData {
    static final int SCHEMA_VERSION = 3;
    static final int GRID_CARDS_PER_PAGE = 6;
    static final int CLOCK_CARDS_PER_PAGE = 4;

    static final String LAYOUT_GRID = "grid";
    static final String LAYOUT_CLOCK = "clock";
    static final String SORT_LOWEST = "lowest";
    static final String SORT_PROVIDER = "provider";

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

        boolean isEstimated() {
            String lower = name == null ? "" : name.toLowerCase(Locale.ROOT);
            return lower.contains("(est") || lower.contains("est.") || lower.contains("estimated");
        }

        boolean isLow() {
            return !unlimited && remaining != null && remaining <= 20.0;
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

        /** Clean, deduplicated label for the single-account detail page. */
        String detailLabel() {
            String key = name == null ? "" : name.trim().toLowerCase(Locale.ROOT);
            String base;
            if ("credits".equals(key) || "credit".equals(key)) {
                base = "基础积分";
            } else if ("extra credits".equals(key)) {
                base = "额外积分（Extra credits）";
            } else if ("total credits".equals(key)) {
                base = "总积分（Total credits）";
            } else {
                return label();
            }
            return qualifier == null ? base : base + " · " + qualifier;
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
            String key = unit.trim().toLowerCase(Locale.ROOT);
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

        String stableKey() {
            return (id != null && !id.isEmpty()) ? id : (provider + "::" + label);
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

        /**
         * Window representing this account on overview cards: lowest non-estimated
         * numeric window first, falling back to estimated windows, then primary().
         */
        Item leadItem() {
            if (items.isEmpty()) return null;
            Item bestReal = null;
            Item bestEst = null;
            for (Item item : items) {
                if (item.unlimited || item.remaining == null) continue;
                if (item.isEstimated()) {
                    if (bestEst == null || compareRemaining(item, bestEst) < 0) bestEst = item;
                } else {
                    if (bestReal == null || compareRemaining(item, bestReal) < 0) bestReal = item;
                }
            }
            if (bestReal != null) return bestReal;
            if (bestEst != null) return bestEst;
            return primary();
        }

        double lowestRemaining() {
            if (isBalance()) return Double.MAX_VALUE;
            Item lead = leadItem();
            if (lead == null || lead.unlimited || lead.remaining == null) return Double.MAX_VALUE;
            return lead.remaining;
        }

        boolean hasLowItem() {
            if (failed) return true;
            for (Item item : items) {
                if (!item.isEstimated() && item.isLow()) return true;
            }
            Item lead = leadItem();
            return lead != null && lead.isLow();
        }

        String statusText() {
            if (failed) return "读取失败";
            if (deferred) return "本轮跳过";
            if (stale) return "缓存";
            return null;
        }
    }

    static final class ProviderOption {
        final String provider;
        final String providerName;
        final int count;

        ProviderOption(String provider, String providerName, int count) {
            this.provider = provider;
            this.providerName = providerName;
            this.count = count;
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
    /** Cards wanted on screen: 1, 2, or every account. */
    int displayLimit = Integer.MAX_VALUE;
    String layoutMode = LAYOUT_GRID;
    String sortMode = SORT_LOWEST;
    String providerFilter = "";
    int page;
    String selectedAccountId;

    static DashboardData parse(String raw) throws Exception {
        return parse(raw, "all", LAYOUT_GRID, SORT_LOWEST, "");
    }

    static DashboardData parse(String raw, String displayMode) throws Exception {
        return parse(raw, displayMode, LAYOUT_GRID, SORT_LOWEST, "");
    }

    static DashboardData parse(String raw, String displayMode, String layoutMode,
                               String sortMode, String providerFilter) throws Exception {
        JSONObject root = new JSONObject(raw);
        DashboardData data = new DashboardData();
        data.displayLimit = limitFor(displayMode);
        data.layoutMode = LAYOUT_CLOCK.equals(layoutMode) ? LAYOUT_CLOCK : LAYOUT_GRID;
        data.sortMode = SORT_PROVIDER.equals(sortMode) ? SORT_PROVIDER : SORT_LOWEST;
        data.providerFilter = providerFilter == null ? "" : providerFilter.trim();
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
        data.refreshRenderKey();
        return data;
    }

    void refreshRenderKey() {
        this.renderKey = key(this);
    }

    private static int limitFor(String mode) {
        if ("one".equals(mode)) return 1;
        if ("two".equals(mode)) return 2;
        return Integer.MAX_VALUE;
    }

    private static int compareRemaining(Item a, Item b) {
        int cmp = Double.compare(a.remaining, b.remaining);
        if (cmp != 0) return cmp;
        return Double.compare(resetTime(a), resetTime(b));
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
                .append('|').append(data.displayLimit)
                .append('|').append(data.layoutMode)
                .append('|').append(data.sortMode)
                .append('|').append(data.providerFilter)
                .append('|').append(data.page)
                .append('|').append(data.selectedAccountId == null ? "" : data.selectedAccountId);
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

    List<ProviderOption> availableProviders() {
        Map<String, ProviderOption> map = new LinkedHashMap<>();
        for (Account account : accounts) {
            String p = account.provider.toLowerCase(Locale.ROOT);
            ProviderOption existing = map.get(p);
            if (existing == null) {
                map.put(p, new ProviderOption(account.provider, account.providerName, 1));
            } else {
                map.put(p, new ProviderOption(existing.provider, existing.providerName, existing.count + 1));
            }
        }
        return new ArrayList<>(map.values());
    }

    List<Account> filteredAccounts() {
        List<Account> filtered = new ArrayList<>();
        String filter = providerFilter == null ? "" : providerFilter.trim();
        for (Account account : accounts) {
            if (!filter.isEmpty() && !filter.equalsIgnoreCase(account.provider)) continue;
            filtered.add(account);
            if (filtered.size() >= displayLimit) break;
        }
        return filtered;
    }

    List<Account> shownAccounts() {
        return filteredAccounts();
    }

    List<Account> orderedAccounts() {
        List<Account> base = filteredAccounts();
        if (base.size() <= 1) return base;

        final Map<Account, Integer> order = new LinkedHashMap<>();
        for (int i = 0; i < base.size(); i++) order.put(base.get(i), i);

        List<Account> sorted = new ArrayList<>(base);
        if (SORT_PROVIDER.equals(sortMode)) {
            Collections.sort(sorted, new Comparator<Account>() {
                @Override public int compare(Account a, Account b) {
                    int p = a.providerName.compareToIgnoreCase(b.providerName);
                    if (p != 0) return p;
                    int r = Double.compare(a.lowestRemaining(), b.lowestRemaining());
                    if (r != 0) return r;
                    return Integer.compare(order.get(a), order.get(b));
                }
            });
            return sorted;
        }

        Collections.sort(sorted, new Comparator<Account>() {
            @Override public int compare(Account a, Account b) {
                if (a.failed != b.failed) return a.failed ? -1 : 1;
                int r = Double.compare(a.lowestRemaining(), b.lowestRemaining());
                if (r != 0) return r;
                Item ia = a.leadItem();
                Item ib = b.leadItem();
                double ra = ia == null ? Double.MAX_VALUE : resetTime(ia);
                double rb = ib == null ? Double.MAX_VALUE : resetTime(ib);
                int rt = Double.compare(ra, rb);
                if (rt != 0) return rt;
                return Integer.compare(order.get(a), order.get(b));
            }
        });

        // In Swiss Grid mode, diversify slots 2..5 on page 1 so 3 identical
        // full-credit accounts don't crowd out other providers on the first screen.
        if (LAYOUT_GRID.equals(layoutMode) && sorted.size() > GRID_CARDS_PER_PAGE && providerFilter.isEmpty()) {
            List<Account> diversified = new ArrayList<>(sorted.size());
            Set<Account> used = new HashSet<>();
            Set<String> seenProviders = new HashSet<>();

            for (int i = 0; i < 2; i++) {
                Account focus = sorted.get(i);
                diversified.add(focus);
                used.add(focus);
                seenProviders.add(focus.provider.toLowerCase(Locale.ROOT));
            }
            for (Account account : sorted) {
                if (diversified.size() >= GRID_CARDS_PER_PAGE) break;
                if (used.contains(account)) continue;
                String p = account.provider.toLowerCase(Locale.ROOT);
                if (!seenProviders.contains(p)) {
                    diversified.add(account);
                    used.add(account);
                    seenProviders.add(p);
                }
            }
            for (Account account : sorted) {
                if (!used.contains(account)) diversified.add(account);
            }
            return diversified;
        }
        return sorted;
    }

    int cardsPerPage() {
        return LAYOUT_CLOCK.equals(layoutMode) ? CLOCK_CARDS_PER_PAGE : GRID_CARDS_PER_PAGE;
    }

    int pageCount() {
        int count = orderedAccounts().size();
        if (count == 0) return 1;
        return (count + cardsPerPage() - 1) / cardsPerPage();
    }

    List<Account> pageAccounts() {
        List<Account> all = orderedAccounts();
        if (all.isEmpty()) return all;
        int perPage = cardsPerPage();
        int safePage = Math.max(0, Math.min(page, pageCount() - 1));
        int from = safePage * perPage;
        if (from >= all.size()) return Collections.emptyList();
        int to = Math.min(all.size(), from + perPage);
        return all.subList(from, to);
    }

    void clampPage() {
        page = Math.max(0, Math.min(page, pageCount() - 1));
        if (selectedAccountId != null && detailAccount() == null) {
            selectedAccountId = null;
        }
    }

    /** The account currently opened in single-account detail view, or null. */
    Account detailAccount() {
        if (selectedAccountId == null || selectedAccountId.isEmpty()) return null;
        for (Account account : orderedAccounts()) {
            if (selectedAccountId.equals(account.stableKey())) return account;
        }
        return null;
    }

    int detailAccountIndex() {
        Account target = detailAccount();
        if (target == null) return -1;
        List<Account> all = orderedAccounts();
        for (int i = 0; i < all.size(); i++) {
            if (all.get(i) == target) return i;
        }
        return -1;
    }

    void openDetail(Account account) {
        selectedAccountId = account == null ? null : account.stableKey();
        refreshRenderKey();
    }

    void closeDetail() {
        selectedAccountId = null;
        refreshRenderKey();
    }

    void goHome() {
        selectedAccountId = null;
        page = 0;
        refreshRenderKey();
    }

    void prevPage() {
        int count = pageCount();
        page = (page - 1 + count) % count;
        refreshRenderKey();
    }

    void nextPage() {
        page = (page + 1) % pageCount();
        refreshRenderKey();
    }

    void prevDetailAccount() {
        List<Account> all = orderedAccounts();
        if (all.isEmpty()) return;
        int idx = detailAccountIndex();
        int nextIdx = idx <= 0 ? all.size() - 1 : idx - 1;
        selectedAccountId = all.get(nextIdx).stableKey();
        refreshRenderKey();
    }

    void nextDetailAccount() {
        List<Account> all = orderedAccounts();
        if (all.isEmpty()) return;
        int idx = detailAccountIndex();
        int nextIdx = (idx + 1) % all.size();
        selectedAccountId = all.get(nextIdx).stableKey();
        refreshRenderKey();
    }

    /**
     * The window the overview leads with: whatever is closest to running out.
     * Real subscription/credit windows take precedence over estimated ones.
     */
    Slot hero() {
        Slot bestReal = null;
        Slot bestEst = null;
        for (Account account : filteredAccounts()) {
            for (Item item : account.items) {
                if (item.remaining == null || item.unlimited) continue;
                if (item.isEstimated()) {
                    if (bestEst == null || compareRemaining(item, bestEst.item) < 0) {
                        bestEst = new Slot(account, item);
                    }
                } else {
                    if (bestReal == null || compareRemaining(item, bestReal.item) < 0) {
                        bestReal = new Slot(account, item);
                    }
                }
            }
        }
        if (bestReal != null) return bestReal;
        if (bestEst != null) return bestEst;
        for (Account account : filteredAccounts()) {
            if (!account.items.isEmpty()) return new Slot(account, account.items.get(0));
        }
        return null;
    }

    private static double resetTime(Item item) {
        return item.resetAt == null ? Double.MAX_VALUE : item.resetAt;
    }

    int providerCount() {
        List<String> seen = new ArrayList<>();
        for (Account account : filteredAccounts()) {
            String key = account.provider.toLowerCase(Locale.ROOT);
            if (!seen.contains(key)) seen.add(key);
        }
        return seen.size();
    }
}
