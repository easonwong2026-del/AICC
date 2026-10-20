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
            testCredentialStorage();
            testRenderAccountStates();
            testPairingSettings();
            sendStatus(0, status);
            finish(Activity.RESULT_OK, new Bundle());
        } catch (Throwable error) {
            status.putString("stack", Log.getStackTraceString(error));
            sendStatus(-2, status);
            finish(Activity.RESULT_CANCELED, status);
        }
    }

    public void testRenderAccountStates() throws Exception {
        for (String scenario : new String[]{"one", "two", "many", "empty", "cached", "unknown", "mixed", "long"}) {
            DashboardData data = parse(scenario);
            File directory = getTargetContext().getExternalFilesDir("visual");
            if (directory == null) throw new AssertionError("Missing output directory");
            runOnMainSync(() -> {
                DashboardView view = new DashboardView(getTargetContext());
                view.layout(0, 0, 758, 1024);
                view.setData(data);
                save(view, new File(directory, scenario + ".png"));
                if (data.pageCount() > 1) {
                    view.nextPage();
                    save(view, new File(directory, scenario + "-page-2.png"));
                }
            });
        }
    }

    /** Schema-3 snapshots, exactly what the bridge answers a v3 client. */
    private DashboardData parse(String scenario) throws Exception {
        boolean many = "many".equals(scenario) || "mixed".equals(scenario);
        int count = many ? 7 : "empty".equals(scenario) ? 0 : "two".equals(scenario) ? 2 : 1;
        JSONArray accounts = new JSONArray();
        for (int index = 0; index < count; index++) {
            boolean balance = "mixed".equals(scenario) && index == 6;
            boolean plan = "mixed".equals(scenario) && index == 5;
            boolean cached = "cached".equals(scenario);
            JSONObject row = new JSONObject()
                    .put("id", "magpie-" + index)
                    .put("provider", balance ? "deepseek" : plan ? "zcode" : index % 2 == 0 ? "codex" : "workbuddy")
                    .put("provider_name", balance ? "DeepSeek" : plan ? "ZCode" : index % 2 == 0 ? "Codex" : "WorkBuddy")
                    .put("account_label", "长服务商名称测试" + (index + 1) + "***")
                    .put("kind", balance ? "balance" : plan ? "plan" : "subscription")
                    .put("plan", plan ? "GLM Coding Plan" : "Free")
                    .put("stale", cached)
                    .put("failed", "unknown".equals(scenario))
                    .put("deferred", false)
                    .put("read_at", 1792000000)
                    .put("balance", balance ? "58.70 CNY" : JSONObject.NULL);
            JSONArray items = new JSONArray();
            if (balance) {
                items.put(new JSONObject().put("name", "balance").put("remaining", JSONObject.NULL));
            } else if (plan) {
                items.put(new JSONObject().put("name", "prompts").put("remaining", 87.5)
                        .put("display", "35 / 40").put("unit", "prompts")
                        .put("reset", "2026-10-21 00:00"));
            } else if ("unknown".equals(scenario)) {
                items.put(new JSONObject().put("name", "5 hours").put("remaining", JSONObject.NULL));
            } else {
                items.put(new JSONObject().put("name", "5 hours").put("remaining", 91 - index * 5)
                        .put("reset", "2026-10-20 19:40"));
                items.put(new JSONObject().put("name", "Credits").put("remaining", 88 - index * 5)
                        .put("display", (4723 - index * 100) + " / 4723").put("unit", "credits"));
            }
            row.put("items", items);
            accounts.put(row);
        }
        JSONObject document = new JSONObject()
                .put("schema_version", 3)
                .put("source", "Magpie")
                .put("available", count > 0)
                .put("state", count == 0 ? "Empty" : "cached".equals(scenario) ? "Cached" : "Connected")
                .put("stale", "cached".equals(scenario))
                .put("updated_at", "2026-10-20 16:30")
                .put("accounts", accounts);
        return DashboardData.parse(document.toString(), "all");
    }

    private void testCredentialStorage() throws Exception {
        android.content.SharedPreferences preferences = getTargetContext().getSharedPreferences("credential-test", 0);
        preferences.edit().clear().putString("host_id", "test-host").commit();
        String token = "isolated-test-credential-never-a-real-secret";
        BridgeCredential.save(preferences, token, "test-host");
        if (!token.equals(BridgeCredential.load(preferences))) throw new AssertionError("Credential roundtrip failed");
        if (preferences.getAll().containsValue(token)) throw new AssertionError("Plaintext credential persisted");
        preferences.edit().putString("host_id", "other-host").commit();
        if (BridgeCredential.load(preferences) != null) throw new AssertionError("Cross-host credential leak");
        preferences.edit().putString("host_id", "test-host").putString("bridge_secret", "tampered").commit();
        if (BridgeCredential.load(preferences) != null) throw new AssertionError("Corrupt credential accepted");
        preferences.edit().clear().commit();
    }

    private void testPairingSettings() throws Exception {
        android.content.Intent intent = new android.content.Intent(getTargetContext(), MainActivity.class);
        intent.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK);
        ActivityMonitor monitor = addMonitor(MainActivity.class.getName(), null, false);
        getTargetContext().startActivity(intent);
        Activity activity = monitor.waitForActivityWithTimeout(10000);
        removeMonitor(monitor);
        if (activity == null) throw new AssertionError("Settings activity did not open");
        runOnMainSync(() -> {
            android.app.Dialog dialog = ((MainActivity) activity).showSettings();
            android.view.View decor = dialog.getWindow().getDecorView();
            int width = (int) (activity.getResources().getDisplayMetrics().widthPixels * .94f);
            int height = (int) (activity.getResources().getDisplayMetrics().heightPixels * .78f);
            decor.measure(android.view.View.MeasureSpec.makeMeasureSpec(width, android.view.View.MeasureSpec.EXACTLY),
                    android.view.View.MeasureSpec.makeMeasureSpec(height, android.view.View.MeasureSpec.EXACTLY));
            decor.layout(0, 0, width, height);
            if (!dialog.isShowing() || !containsText(decor, "Magpie 墨水屏伴侣") || !containsText(decor, "配对这台电脑")) {
                throw new AssertionError("Pairing dialog missing");
            }
            Bitmap screenshot = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888);
            screenshot.eraseColor(android.graphics.Color.WHITE);
            decor.draw(new Canvas(screenshot));
            try (FileOutputStream output = new FileOutputStream(new File(getTargetContext().getExternalFilesDir("visual"), "pairing-settings.png"))) {
                if (!screenshot.compress(Bitmap.CompressFormat.PNG, 100, output)) throw new AssertionError("Settings PNG failed");
            } catch (Exception error) { throw new AssertionError(error); }
            finally {
                screenshot.recycle();
                dialog.dismiss();
                activity.finish();
            }
        });
    }

    private boolean containsText(android.view.View view, String expected) {
        if (view instanceof android.widget.TextView
                && ((android.widget.TextView) view).getText().toString().contains(expected)) return true;
        if (view instanceof android.view.ViewGroup) {
            android.view.ViewGroup group = (android.view.ViewGroup) view;
            for (int index = 0; index < group.getChildCount(); index++) {
                if (containsText(group.getChildAt(index), expected)) return true;
            }
        }
        return false;
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
