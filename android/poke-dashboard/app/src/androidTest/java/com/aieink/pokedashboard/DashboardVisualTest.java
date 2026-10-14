package com.aieink.pokedashboard;

import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.test.InstrumentationTestCase;

import org.json.JSONArray;
import org.json.JSONObject;
import java.io.File;
import java.io.FileOutputStream;

public final class DashboardVisualTest extends InstrumentationTestCase {
    public void testRenderAccountStates() throws Exception {
        for (String scenario : new String[]{"one", "two", "many", "empty", "cached", "unknown"}) {
            int count = "many".equals(scenario) ? 7 : "two".equals(scenario) ? 2 : "empty".equals(scenario) ? 0 : 1;
            JSONArray accounts = new JSONArray();
            for (int index = 0; index < count; index++) {
                JSONObject row = new JSONObject().put("id", "magpie-" + index).put("label", "Codex · " + (index + 1));
                row.put("weekly", new JSONObject().put("remaining", "unknown".equals(scenario) ? JSONObject.NULL : 88 - index * 10)
                        .put("reset", "2026-10-20 16:10"));
                row.put("five_hour", new JSONObject().put("remaining", 91 - index * 10).put("reset", "2026-10-14 16:58"));
                accounts.put(row);
            }
            JSONObject codex = new JSONObject().put("source", "Magpie").put("available", count > 0)
                    .put("stale", "cached".equals(scenario)).put("updated_at", "2026-10-14 13:32")
                    .put("accounts", accounts);
            DashboardData data = DashboardData.parse(new JSONObject().put("codex", codex)
                    .put("codex_account_display_mode", "all").toString());
            File directory = getInstrumentation().getTargetContext().getExternalFilesDir("visual");
            assertNotNull(directory);
            getInstrumentation().runOnMainSync(() -> {
                DashboardView view = new DashboardView(getInstrumentation().getTargetContext());
                view.layout(0, 0, 758, 1024);
                view.setData(data);
                save(view, new File(directory, scenario + ".png"));
                if (count > 4) {
                    view.nextAccountPage();
                    save(view, new File(directory, "many-page-2.png"));
                }
            });
        }
    }

    private void save(DashboardView view, File file) {
        Bitmap image = Bitmap.createBitmap(758, 1024, Bitmap.Config.ARGB_8888);
        view.draw(new Canvas(image));
        assertEquals(758, image.getWidth());
        assertEquals(1024, image.getHeight());
        assertTrue(image.getPixel(0, 0) != image.getPixel(34, 98));
        try (FileOutputStream output = new FileOutputStream(file)) {
            assertTrue(image.compress(Bitmap.CompressFormat.PNG, 100, output));
        } catch (Exception error) {
            throw new AssertionError(error);
        } finally {
            image.recycle();
        }
    }
}
