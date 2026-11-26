package com.aieink.pokedashboard;

import org.junit.Test;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertFalse;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

public final class DashboardDataTest {
    private static String card(String provider, String kind, String label, String items) {
        return "{\"id\":\"magpie-" + provider + "-" + label + "\",\"provider\":\"" + provider + "\","
                + "\"provider_name\":\"" + DashboardData.providerName(provider) + "\","
                + "\"account_label\":\"" + label + "\","
                + "\"kind\":\"" + kind + "\",\"plan\":\"Free\",\"stale\":false,\"failed\":false,"
                + "\"deferred\":false,\"read_at\":1,\"balance\":null,\"balance_parts\":[],"
                + "\"items\":[" + items + "]}";
    }

    private static String snapshot(String... cards) {
        return "{\"schema_version\":3,\"source\":\"Magpie\",\"available\":true,\"state\":\"Connected\","
                + "\"stale\":false,\"providers\":[],\"refresh_seconds\":300,\"updated_at\":\"2026-10-20 16:00\","
                + "\"accounts\":[" + String.join(",", cards) + "]}";
    }

    @Test public void everyKindOfQuotaSurvivesTheBridge() throws Exception {
        String body = snapshot(
                card("codex", "subscription", "a***",
                        "{\"name\":\"5 hours\",\"remaining\":91,\"reset\":\"2026-10-20 19:40\"},"
                                + "{\"name\":\"7 days\",\"remaining\":88}"),
                card("workbuddy", "subscription", "b***",
                        "{\"name\":\"Credits\",\"remaining\":99.95,\"display\":\"2.27 / 4723\",\"unit\":\"credits\"}"),
                card("zcode", "plan", "c***", "{\"name\":\"prompts\",\"remaining\":87.5,\"display\":\"35 / 40\"}"),
                card("deepseek", "balance", "d***", ""));
        DashboardData data = DashboardData.parse(body, "all");
        assertEquals(4, data.accounts.size());
        assertEquals("Codex · a***", data.accounts.get(0).title());
        assertEquals(2, data.accounts.get(0).items.size());
        assertEquals("91%", data.accounts.get(0).primary().primaryText());
        assertEquals("5 hours", data.accounts.get(0).primary().name);
        assertEquals("2.27 / 4723", data.accounts.get(1).primary().primaryText());
        assertFalse("A credit count is not a percentage", data.accounts.get(1).primary().isPercent());
        assertEquals("plan", data.accounts.get(2).kind);
        assertEquals("prompts", data.accounts.get(2).primary().name);
        assertTrue(data.accounts.get(3).isBalance());
    }

    @Test public void creditsReportWhatIsLeftNotUsedOverTotal() throws Exception {
        String body = snapshot(card("workbuddy", "subscription", "a***",
                "{\"name\":\"Credits\",\"remaining\":99.5,\"display\":\"22 / 4727\","
                        + "\"remaining_text\":\"4705\",\"unit\":\"credits\",\"limit\":4727}"));
        DashboardData.Item item = DashboardData.parse(body, "all").accounts.get(0).primary();
        assertEquals("4705", item.primaryText());
        assertEquals("22 / 4727", item.display);
    }

    @Test public void unitsAreShownInChineseLikeTheWidget() throws Exception {
        String body = snapshot(card("workbuddy", "subscription", "a***",
                "{\"name\":\"Credits\",\"remaining\":99.5,\"remaining_text\":\"4705\",\"unit\":\"credits\"}"));
        DashboardData.Account account = DashboardData.parse(body, "all").accounts.get(0);
        assertEquals("积分", account.primary().unitLabel());

        DashboardData.Item prompts = new DashboardData.Item();
        prompts.unit = "prompts";
        assertEquals("次", prompts.unitLabel());
        DashboardData.Item tokens = new DashboardData.Item();
        tokens.unit = "tokens";
        assertEquals("tokens", tokens.unitLabel());
        assertNull(new DashboardData.Item().unitLabel());
    }

    @Test public void balanceIsMoneyAndNeverAShare() throws Exception {
        String body = snapshot("{\"id\":\"magpie-d\",\"provider\":\"deepseek\",\"provider_name\":\"DeepSeek\","
                + "\"account_label\":\"d***\",\"kind\":\"balance\",\"balance\":\"58.70 CNY\",\"items\":[],"
                + "\"balance_parts\":[{\"label\":\"paid\",\"text\":\"50.00\"}],\"stale\":false,\"failed\":false,"
                + "\"deferred\":false}");
        DashboardData.Account account = DashboardData.parse(body, "all").accounts.get(0);
        assertEquals("58.70 CNY", account.balanceText());
        assertTrue(account.items.isEmpty());
        assertNull(account.primary());
    }

    @Test public void dualModePagingAndDetailNavigationWorkCleanly() throws Exception {
        String wide = "{\"name\":\"Gemini 7 days\",\"remaining\":80},"
                + "{\"name\":\"Gemini 5 hours\",\"remaining\":70},"
                + "{\"name\":\"Claude 7 days\",\"remaining\":60}";
        String body = snapshot(
                card("codex", "subscription", "a***", "{\"name\":\"7 days\",\"remaining\":10},"
                        + "{\"name\":\"5 hours\",\"remaining\":20}"),
                card("antigravity", "subscription", "b***", wide),
                card("kiro", "subscription", "c***", "{\"name\":\"Monthly\",\"remaining\":90}"),
                card("workbuddy", "subscription", "d***", "{\"name\":\"Credits\",\"remaining\":100}"),
                card("workbuddy", "subscription", "e***", "{\"name\":\"Credits\",\"remaining\":100}"),
                card("qoder", "subscription", "f***", "{\"name\":\"Credits\",\"remaining\":82}"),
                card("zcode", "plan", "g***", "{\"name\":\"prompts\",\"remaining\":74}"));
        DashboardData grid = DashboardData.parse(body, "all",
                DashboardData.LAYOUT_GRID, DashboardData.SORT_LOWEST, "");
        assertEquals(2, grid.pageCount());
        assertEquals(6, grid.pageAccounts().size());
        assertNull(grid.detailAccount());

        grid.openDetail(grid.orderedAccounts().get(1));
        assertNotEquals(null, grid.detailAccount());
        assertEquals(3, grid.detailAccount().items.size());
        grid.nextDetailAccount();
        grid.closeDetail();
        assertNull(grid.detailAccount());

        DashboardData clock = DashboardData.parse(body, "all",
                DashboardData.LAYOUT_CLOCK, DashboardData.SORT_LOWEST, "");
        assertEquals(2, clock.pageCount());
        assertEquals(4, clock.pageAccounts().size());

        DashboardData one = DashboardData.parse(body, "one");
        assertEquals(1, one.pageCount());
        one.nextPage();
        assertEquals(0, one.page);
    }

    @Test public void theOverviewLeadsWithRealWindowsBeforeEstimatedOnes() throws Exception {
        String body = snapshot(
                card("workbuddy", "subscription", "est***",
                        "{\"name\":\"deepseek-v4.1 tokens (est.)\",\"remaining\":5}"),
                card("codex", "subscription", "a***", "{\"name\":\"7 days\",\"remaining\":88}"),
                card("zcode", "plan", "c***", "{\"name\":\"prompts\",\"remaining\":12}"));
        DashboardData.Slot hero = DashboardData.parse(body, "all").hero();
        assertEquals("zcode", hero.account.provider);
        assertEquals(12.0, hero.item.remaining, 0.001);
        assertTrue(DashboardData.parse(body, "all").accounts.get(0).primary().isEstimated());
    }

    @Test public void pageOneDiversifiesProvidersAndFiltersCleanly() throws Exception {
        String body = snapshot(
                card("codex", "subscription", "a***", "{\"name\":\"7 days\",\"remaining\":18}"),
                card("antigravity", "subscription", "b***", "{\"name\":\"7 days\",\"remaining\":59}"),
                card("workbuddy", "subscription", "w1***", "{\"name\":\"Credits\",\"remaining\":100}"),
                card("workbuddy", "subscription", "w2***", "{\"name\":\"Credits\",\"remaining\":100}"),
                card("workbuddy", "subscription", "w3***", "{\"name\":\"Credits\",\"remaining\":100}"),
                card("qoder", "subscription", "q***", "{\"name\":\"Credits\",\"remaining\":100}"),
                card("zcode", "plan", "z***", "{\"name\":\"prompts\",\"remaining\":100}"));
        DashboardData grid = DashboardData.parse(body, "all",
                DashboardData.LAYOUT_GRID, DashboardData.SORT_LOWEST, "");
        java.util.List<DashboardData.Account> p1 = grid.pageAccounts();
        assertEquals(6, p1.size());
        // Slots 0..1 are lowest; slots 2..5 pick distinct providers before repeating WorkBuddy.
        assertEquals("codex", p1.get(0).provider);
        assertEquals("antigravity", p1.get(1).provider);
        boolean sawQoder = false;
        boolean sawZcode = false;
        for (DashboardData.Account acc : p1) {
            if ("qoder".equals(acc.provider)) sawQoder = true;
            if ("zcode".equals(acc.provider)) sawZcode = true;
        }
        assertTrue("Expected Qoder on page 1", sawQoder);
        assertTrue("Expected ZCode on page 1", sawZcode);

        DashboardData filtered = DashboardData.parse(body, "all",
                DashboardData.LAYOUT_GRID, DashboardData.SORT_PROVIDER, "workbuddy");
        assertEquals(3, filtered.orderedAccounts().size());
        assertEquals(1, filtered.pageCount());
    }

    @Test public void detailLabelsNormalizeCreditVariants() {
        DashboardData.Item base = new DashboardData.Item();
        base.name = "Credits";
        assertEquals("基础积分", base.detailLabel());
        DashboardData.Item extra = new DashboardData.Item();
        extra.name = "Extra credits";
        assertEquals("额外积分（Extra credits）", extra.detailLabel());
        DashboardData.Item total = new DashboardData.Item();
        total.name = "Total credits";
        assertEquals("总积分（Total credits）", total.detailLabel());
    }

    @Test public void resetsAreShownAsACountdownAndWindowsAreNamedInChinese() throws Exception {
        long now = 1_800_000_000L;
        DashboardData.Item item = new DashboardData.Item();
        item.name = "7 days";
        item.resetAt = (double) now + 3 * 86400 + 4 * 3600;
        assertEquals("7 天", item.windowLabel());
        assertEquals("3 天 4 时后重置", item.countdown(now));
        item.resetAt = (double) now + 4 * 3600 + 12 * 60;
        assertEquals("4 时 12 分后重置", item.countdown(now));
        item.resetAt = (double) now + 90;
        assertEquals("1 分后重置", item.countdown(now));
        item.resetAt = (double) now - 5;
        assertEquals("即将重置", item.countdown(now));
        item.resetAt = null;
        item.reset = "2026-10-22 14:14";
        assertEquals("2026-10-22 14:14", item.countdown(now));

        DashboardData.Item credits = new DashboardData.Item();
        credits.name = "Credits";
        assertEquals("积分", credits.windowLabel());
        DashboardData.Item hours = new DashboardData.Item();
        hours.name = "5 hours";
        assertEquals("5 小时", hours.windowLabel());
        DashboardData.Item custom = new DashboardData.Item();
        custom.name = "Gemini 3.8 Flash";
        assertEquals("Gemini 3.8 Flash", custom.windowLabel());
    }

    @Test public void countedWindowsCarryTheirTotal() throws Exception {
        DashboardData.Item item = new DashboardData.Item();
        item.unit = "credits";
        item.limit = 4727.0;
        assertEquals("共 4727", item.totalText());
        item.unlimited = true;
        assertNull(item.totalText());
        item.unlimited = false;
        item.unit = null;
        assertNull(item.totalText());
    }

    @Test public void unknownProvidersStayAndUnknownKindsAreSkipped() throws Exception {
        String body = snapshot(
                card("brandnew-vendor", "subscription", "x***", "{\"name\":\"Monthly\",\"remaining\":90}"),
                card("mystery", "giftcard", "y***", "{\"name\":\"Odd\",\"remaining\":10}"));
        DashboardData data = DashboardData.parse(body, "all");
        assertEquals(1, data.accounts.size());
        assertEquals("brandnew-vendor", data.accounts.get(0).provider);
    }

    @Test public void missingQuotaIsUnknownNotZero() throws Exception {
        String body = snapshot(
                card("codex", "subscription", "a***", "{\"name\":\"5 hours\",\"remaining\":null}"),
                card("workbuddy", "subscription", "b***", ""),
                card("zcode", "plan", "c***", "{\"name\":\"prompts\",\"unlimited\":true}"));
        DashboardData data = DashboardData.parse(body, "all");
        assertNull(data.accounts.get(0).primary().remaining);
        assertEquals("—", data.accounts.get(0).primary().primaryText());
        assertNull(data.accounts.get(1).primary());
        assertEquals(0, data.accounts.get(1).items.size());
        assertTrue(data.accounts.get(2).primary().unlimited);
        assertEquals("∞", data.accounts.get(2).primary().primaryText());
    }

    @Test public void malformedNumbersAndTimesAreIgnored() throws Exception {
        for (String value : new String[]{"null", "true", "\"bad\"", "-1", "240"}) {
            String body = snapshot(card("codex", "subscription", "a***",
                    "{\"name\":\"5 hours\",\"remaining\":" + value + "}"));
            assertNull(value, DashboardData.parse(body, "all").accounts.get(0).primary().remaining);
        }
        String body = snapshot(card("codex", "subscription", "a***", "{\"name\":\"5 hours\",\"used\":80}"));
        assertEquals("20%", DashboardData.parse(body, "all").accounts.get(0).primary().primaryText());
    }

    @Test public void statusAndPlanAreShownButNeverInvented() throws Exception {
        String body = "{\"schema_version\":3,\"available\":true,\"state\":\"Cached\",\"stale\":true,"
                + "\"updated_at\":\"2026-10-20 16:00\",\"accounts\":["
                + "{\"id\":\"m1\",\"provider\":\"codex\",\"provider_name\":\"Codex\",\"account_label\":\"a***\","
                + "\"kind\":\"subscription\",\"items\":[],\"stale\":true,\"failed\":false,\"deferred\":false},"
                + "{\"id\":\"m2\",\"provider\":\"codex\",\"provider_name\":\"Codex\",\"account_label\":\"b***\","
                + "\"kind\":\"subscription\",\"items\":[],\"stale\":true,\"failed\":true,\"deferred\":false}]}";
        DashboardData data = DashboardData.parse(body, "all");
        assertEquals("Cached", data.state);
        assertEquals("缓存", data.accounts.get(0).statusText());
        assertEquals("读取失败", data.accounts.get(1).statusText());
        DashboardData empty = DashboardData.parse("{\"schema_version\":3,\"available\":false,"
                + "\"state\":\"Empty\",\"accounts\":[]}", "all");
        assertTrue(empty.accounts.isEmpty());
        assertFalse(empty.available);
    }

    @Test public void repaintOnlyHappensOnRealChange() throws Exception {
        String original = snapshot(card("codex", "subscription", "a***", "{\"name\":\"7 days\",\"remaining\":88}"));
        DashboardData first = DashboardData.parse(original, "all");
        DashboardData sameQuotaNewTime = DashboardData.parse(
                original.replace("2026-10-20 16:00", "2026-10-20 16:05"), "all");
        assertEquals(first.renderKey, sameQuotaNewTime.renderKey);
        assertNotEquals(first.updatedAt, sameQuotaNewTime.updatedAt);
        assertNotEquals(first.renderKey, DashboardData.parse(original.replace("88", "87"), "all").renderKey);
        assertNotEquals(first.renderKey, DashboardData.parse(original, "one").renderKey);
        assertNotEquals(first.renderKey, DashboardData.parse(
                original.replace("\"stale\":false", "\"stale\":true"), "all").renderKey);
    }

    @Test public void offlineCacheKeepsTheLastReading() throws Exception {
        String original = snapshot(card("codex", "subscription", "a***", "{\"name\":\"7 days\",\"remaining\":88}"));
        DashboardData cached = DashboardData.parse(original, "all");
        assertFalse(cached.offline);
        assertEquals("88%", cached.accounts.get(0).primary().primaryText());
    }

    @Test public void olderBridgeShapeIsReadExplicitly() throws Exception {
        String legacy = "{\"codex\":{\"available\":true,\"stale\":false,\"accounts\":["
                + "{\"id\":\"a\",\"label\":\"Codex · a***\",\"weekly\":{\"remaining\":59},"
                + "\"five_hour\":{\"remaining\":0}}]}}";
        DashboardData data = DashboardData.parse(legacy, "all");
        assertEquals(1, data.accounts.size());
        assertEquals("codex", data.accounts.get(0).provider);
        assertEquals("Codex", data.accounts.get(0).providerName);
        assertEquals("59%", data.accounts.get(0).primary().primaryText());
        assertEquals(0.0, data.accounts.get(0).secondary().remaining, 0.001);
        assertTrue(data.accounts.get(0).secondary().isPercent());
    }

    @Test public void brokenPayloadsAreRejected() {
        for (String body : new String[]{"", "not json", "[]", "{\"schema_version\":3}"}) {
            boolean rejected = false;
            try {
                DashboardData data = DashboardData.parse(body, "all");
                rejected = data.accounts.isEmpty();
            } catch (Exception expected) {
                rejected = true;
            }
            assertTrue(body, rejected);
        }
    }
}
