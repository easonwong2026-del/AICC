package com.aieink.pokedashboard;

import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.app.Activity;
import android.app.Instrumentation;
import android.os.Bundle;
import android.util.Log;

import org.json.JSONArray;
import org.json.JSONObject;
import java.io.File;
import java.io.FileOutputStream;

public final class DashboardVisualTest extends Instrumentation {
    @Override public void onCreate(Bundle arguments) {
        super.onCreate(arguments);
        start();
    }

    @Override public void onStart() {
        Bundle status = new Bundle();
        status.putString("class", getClass().getName());
        status.putString("test", "testRenderAccountStates");
        status.putInt("numtests", 1);
        status.putInt("current", 1);
        sendStatus(1, status);
        try {
            testRenderAccountStates();
            sendStatus(0, status);
            finish(Activity.RESULT_OK, new Bundle());
        } catch (Throwable error) {
            status.putString("stack", Log.getStackTraceString(error));
            sendStatus(-2, status);
            finish(Activity.RESULT_CANCELED, status);
        }
    }

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
            File directory = getTargetContext().getExternalFilesDir("visual");
            if (directory == null) throw new AssertionError("Missing output directory");
            runOnMainSync(() -> {
                DashboardView view = new DashboardView(getTargetContext());
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
        if (image.getWidth() != 758 || image.getHeight() != 1024 || image.getPixel(0, 0) == image.getPixel(34, 98)) {
            throw new AssertionError("Missing dashboard frame");
        }
        try (FileOutputStream output = new FileOutputStream(file)) {
            if (!image.compress(Bitmap.CompressFormat.PNG, 100, output)) throw new AssertionError("PNG encoding failed");
        } catch (Exception error) {
            throw new AssertionError(error);
        } finally {
            image.recycle();
        }
    }
}
