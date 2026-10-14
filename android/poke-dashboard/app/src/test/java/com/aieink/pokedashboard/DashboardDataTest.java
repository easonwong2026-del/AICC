package com.aieink.pokedashboard;

import org.junit.Test;
import static org.junit.Assert.*;

public final class DashboardDataTest {
    @Test public void unknownQuotaIsNotZero() throws Exception {
        DashboardData data = DashboardData.parse("{\"codex\":{\"weekly\":{\"remaining\":null},\"five_hour\":{\"remaining\":0}}}");
        assertNull(data.weeklyRemaining);
        assertNull(data.accounts.get(0).weekly);
        assertEquals(Integer.valueOf(0), data.accounts.get(0).fiveHour);
    }

    @Test public void poolRespectsPinnedOrderAndDisplayPreference() throws Exception {
        String pool = "\"codex\":{\"source\":\"Magpie\",\"stale\":true,\"selection_mode\":\"pinned\","
                + "\"active_account_id\":\"b\",\"accounts\":[{\"id\":\"a\",\"weekly\":{\"remaining\":88}},"
                + "{\"id\":\"b\",\"weekly\":{\"remaining\":59}},{\"id\":\"c\"}]}";
        DashboardData data = DashboardData.parse("{" + pool + ",\"codex_account_display_mode\":\"one\"}");
        assertEquals(3, data.totalAccounts);
        assertEquals(1, data.accounts.size());
        assertEquals("b", data.accounts.get(0).id);
        assertEquals(Integer.valueOf(59), data.accounts.get(0).weekly);
        assertTrue(data.accounts.get(0).stale);
        assertEquals(3, DashboardData.parse("{" + pool + ",\"codex_account_display_mode\":\"all\"}").accounts.size());
        assertEquals(2, DashboardData.parse("{" + pool + "}").accounts.size());
    }

    @Test public void emptyPoolAndMalformedPercentStayUnknown() throws Exception {
        assertTrue(DashboardData.parse("{\"codex\":{\"accounts\":[]}}").accounts.isEmpty());
        for (String value : new String[]{"null", "true", "\"bad\""}) {
            assertNull(DashboardData.parse("{\"codex\":{\"weekly\":{\"remaining\":" + value + "}}}").accounts.get(0).weekly);
        }
        assertEquals(Integer.valueOf(100), DashboardData.parse("{\"codex\":{\"weekly\":{\"remaining\":110}}}").accounts.get(0).weekly);
    }

    @Test public void unrelatedCollectorsDoNotRepaintCodex() throws Exception {
        String codex = "\"codex\":{\"source\":\"Magpie\",\"weekly\":{\"remaining\":88}}";
        assertEquals(DashboardData.parse("{" + codex + ",\"fetched_at\":1}").renderKey,
                DashboardData.parse("{" + codex + ",\"fetched_at\":2,\"system\":{\"cpu\":\"20%\"}}").renderKey);
    }

    @Test public void sourceTimestampDoesNotRepaintButQuotaAndCacheStateDo() throws Exception {
        String original = "{\"codex\":{\"source\":\"Magpie\",\"updated_at\":\"first\",\"weekly\":{\"remaining\":88}}}";
        DashboardData first = DashboardData.parse(original);
        DashboardData next = DashboardData.parse(original.replace("first", "next"));
        assertEquals(first.renderKey, next.renderKey);
        assertNotEquals(first.updatedAt, next.updatedAt);
        assertNotEquals(first.renderKey, DashboardData.parse(original.replace("88", "87")).renderKey);
        assertNotEquals(first.renderKey, DashboardData.parse(original.replace("\"source\"", "\"stale\":true,\"source\"")).renderKey);
    }
}
