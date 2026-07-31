package com.aieink.pokedashboard;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.Locale;

final class DashboardData {
    String updatedAt = "--";
    String codexState = "Connecting";
    String codexSource = "Codex";
    Integer fiveHourRemaining;
    String fiveHourReset = "--";
    Integer weeklyRemaining;
    String weeklyReset = "--";
    boolean resetCreditsProvided;
    Integer resetCreditsCount;
    String resetCreditsExpiry = "--";
    int limitBucketCount;
    String workBuddyPoints = "--";
    String workBuddyUsed = "--";
    String workBuddyState = "--";
    boolean workBuddyStale;
    String workBuddyUpdatedAt = "--";
    Double workBuddyUpdatedEpoch;
    Long workBuddyAgeSeconds;
    String workBuddyErrorCode = "";
    String deepSeekBalance = "--";
    String deepSeekUsage = "--";
    String deepSeekCurrency = "";
    String deepSeekState = "--";
    String systemState = "--";
    String systemLabel = "--";
    String cpu = "--";
    String ram = "--";
    String gpu = "";
    int refreshingCollectors;
    int failedCollectors;
    boolean offline;

    static DashboardData parse(String raw) throws Exception {
        JSONObject root = new JSONObject(raw);
        DashboardData data = new DashboardData();
        data.updatedAt = root.optString("updated_at", "--");

        JSONObject codex = root.optJSONObject("codex");
        if (codex != null) {
            boolean connected = codex.optBoolean("available", false);
            boolean stale = codex.optBoolean("stale", false);
            data.codexState = connected ? (stale ? "CACHED" : "CONNECTED") : codex.optString("state", "OFFLINE");
            data.codexSource = codex.optString("source", "Codex");
            JSONObject five = codex.optJSONObject("five_hour");
            if (five != null && five.has("remaining")) {
                data.fiveHourRemaining = five.optInt("remaining");
                data.fiveHourReset = five.optString("reset", "--");
            }
            JSONObject weekly = codex.optJSONObject("weekly");
            if (weekly != null && weekly.has("remaining")) {
                data.weeklyRemaining = weekly.optInt("remaining");
                data.weeklyReset = weekly.optString("reset", "--");
            }
            JSONObject resetCredits = codex.optJSONObject("reset_credits");
            if (resetCredits != null) {
                data.resetCreditsProvided = resetCredits.optBoolean("provided", false);
                if (data.resetCreditsProvided && !resetCredits.isNull("available_count")) {
                    data.resetCreditsCount = resetCredits.optInt("available_count");
                }
                data.resetCreditsExpiry = resetCredits.optString("next_expiry", "--");
            }
            JSONArray limitBuckets = codex.optJSONArray("limit_buckets");
            data.limitBucketCount = limitBuckets == null ? 0 : limitBuckets.length();
        }

        JSONObject workBuddy = root.optJSONObject("workbuddy");
        if (workBuddy != null) {
            data.workBuddyPoints = number(workBuddy, "points");
            data.workBuddyUsed = workBuddy.has("auto_used_credits")
                    ? number(workBuddy, "auto_used_credits") : number(workBuddy, "used_points");
            data.workBuddyState = workBuddy.optString("balance_state", "MANUAL").toUpperCase(Locale.ROOT);
            data.workBuddyStale = workBuddy.optBoolean("balance_stale", false);
            data.workBuddyUpdatedAt = workBuddy.optString("balance_updated_at", "--");
            if (!workBuddy.isNull("balance_updated_epoch")) {
                data.workBuddyUpdatedEpoch = workBuddy.optDouble("balance_updated_epoch");
            }
            if (!workBuddy.isNull("balance_age_seconds")) {
                data.workBuddyAgeSeconds = workBuddy.optLong("balance_age_seconds");
            }
            data.workBuddyErrorCode = workBuddy.optString("balance_error_code", "");
        }

        JSONObject deepSeek = root.optJSONObject("deepseek");
        if (deepSeek != null) {
            data.deepSeekState = deepSeek.optString("status", "--").toUpperCase(Locale.ROOT);
            JSONArray balances = deepSeek.optJSONArray("balances");
            JSONObject balance = balances != null && balances.length() > 0 ? balances.optJSONObject(0) : null;
            if (balance != null) {
                data.deepSeekBalance = balance.optString("total_balance", "--");
                data.deepSeekCurrency = balance.optString("currency", "");
            }
            JSONArray usage = deepSeek.optJSONArray("usage");
            JSONObject used = usage != null && usage.length() > 0 ? usage.optJSONObject(0) : null;
            if (used != null) {
                data.deepSeekUsage = compactDecimal(used.optString("used_today", "--"));
            }
        }

        JSONObject system = root.optJSONObject("system");
        if (system != null) {
            data.systemState = system.optString("status", "--");
            data.systemLabel = system.optString("label", "--");
            data.cpu = system.optString("cpu", "--");
            data.ram = system.optString("ram", "--");
            data.gpu = system.optString("gpu", "");
        }

        JSONObject collection = root.optJSONObject("collection");
        if (collection != null) {
            JSONArray names = collection.names();
            for (int index = 0; names != null && index < names.length(); index++) {
                JSONObject item = collection.optJSONObject(names.optString(index));
                String state = item == null ? "" : item.optString("state", "");
                if ("refreshing".equals(state)) data.refreshingCollectors++;
                if ("error".equals(state)) data.failedCollectors++;
            }
        }
        return data;
    }

    private static String number(JSONObject object, String key) {
        Object value = object.opt(key);
        return value == null || value == JSONObject.NULL ? "--" : String.valueOf(value);
    }

    private static String compactDecimal(String value) {
        if (value == null || value.indexOf('.') < 0) return value;
        int end = value.length();
        while (end > 0 && value.charAt(end - 1) == '0') end--;
        if (end > 0 && value.charAt(end - 1) == '.') end--;
        return end == 0 ? "0" : value.substring(0, end);
    }
}
