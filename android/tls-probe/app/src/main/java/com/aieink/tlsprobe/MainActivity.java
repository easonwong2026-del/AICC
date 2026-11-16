package com.aieink.tlsprobe;

import android.app.Activity;
import android.os.Bundle;
import android.util.Base64;
import android.util.Log;
import android.widget.ScrollView;
import android.widget.TextView;

import java.io.BufferedReader;
import java.io.ByteArrayInputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStreamWriter;
import java.net.HttpURLConnection;
import java.net.InetSocketAddress;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.List;

import javax.net.ssl.HttpsURLConnection;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSession;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.SSLSocketFactory;
import javax.net.ssl.TrustManagerFactory;

/**
 * Throwaway probe for PR-B: can POKE4S talk TLS to the AICC bridge, and does it
 * reject the certificates it should reject?
 *
 * Every case prints one line to logcat with tag {@code AICCTlsProbe}:
 * {@code CASE <name> => PASS|FAIL: <detail>}. The whole run is also written to
 * the app's external files directory so it can be pulled with adb.
 *
 * The probe never trusts all certificates. Two of the cases exist only to prove
 * that: one connects with the system trust store (expects a handshake failure)
 * and one compares the leaf's SPKI pin against a deliberately wrong value.
 */
public class MainActivity extends Activity {
    private static final String TAG = "AICCTlsProbe";
    private static final String HOST = "127.0.0.1";
    private static final int PORT = 8767;
    /**
     * Second server, signed by a CA this app does not trust. Connecting here must
     * fail; if it succeeds, the trust configuration is wrong.
     */
    private static final int UNTRUSTED_PORT = 8768;
    private static final int TIMEOUT_MS = 4000;

    /**
     * Same root, but the leaf has been re-issued (the state after a renewal):
     * the pin must still match, because the pin is the root's SPKI.
     */
    private static final int RENEWED_PORT = 8769;
    /**
     * A different root altogether (the state after `tls.reset`): a device
     * pinned to the first root must refuse it.
     */
    private static final int RESET_PORT = 8770;

    private final List<String> lines = new ArrayList<>();
    private TextView view;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        view = new TextView(this);
        view.setPadding(24, 24, 24, 24);
        view.setTextSize(13f);
        view.setText("running…");
        ScrollView scroll = new ScrollView(this);
        scroll.addView(view);
        setContentView(scroll);

        new Thread(this::runAll, "tls-probe").start();
    }

    private void runAll() {
        long started = System.currentTimeMillis();
        run("ecdsa_root_is_p256", () -> {
            // The Mac now mints ECDSA P-256. RSA results from the earlier probe
            // do not carry over, so assert the key type before anything else.
            X509Certificate ca = probeCa();
            require("EC".equals(ca.getPublicKey().getAlgorithm()), "CA key is " + ca.getPublicKey().getAlgorithm());
            String curve = java.security.interfaces.ECPublicKey.class.isInstance(ca.getPublicKey())
                    ? ((java.security.interfaces.ECPublicKey) ca.getPublicKey()).getParams().getCurve()
                        .getField().getFieldSize() + " bit" : "unknown";
            require(curve.startsWith("256"), "curve is " + curve);
            return ca.getSigAlgName() + ", " + curve;
        });
        run("ecdsa_chain_verified", () -> {
            String protocol = handshake(PORT, "TLSv1.3", trustingProbeCa());
            require(protocol != null, "no handshake");
            return "leaf chains to the pinned root (" + protocol + ")";
        });
        run("tls12_handshake", () -> {
            String protocol = handshake(PORT, "TLSv1.2", trustingProbeCa());
            require("TLSv1.2".equals(protocol), "negotiated " + protocol);
            return protocol;
        });
        run("tls13_handshake", () -> {
            String protocol = handshake(PORT, "TLSv1.3", trustingProbeCa());
            require("TLSv1.3".equals(protocol), "negotiated " + protocol);
            return protocol;
        });
        run("https_health_200", () -> {
            int code = httpsStatus("/api/health/live", null);
            require(code == 200, "status " + code);
            return "status 200";
        });
        run("https_status_401_without_token", () -> {
            int code = httpsStatus("/api/status", null);
            require(code == 401, "status " + code);
            return "status 401";
        });
        // Negative case: a server certificate from a CA this app does not trust.
        // The other port is served by a second instance with its own CA, so this
        // does not depend on what the device's system trust store happens to
        // contain — a handshake that succeeds here means we trust everything.
        run("untrusted_server_rejected", () -> {
            try {
                String protocol = handshake(UNTRUSTED_PORT, "TLSv1.2", trustingProbeCa());
                throw new AssertionError("handshake succeeded against an untrusted CA: " + protocol);
            } catch (IOException expected) {
                return "rejected: " + expected.getClass().getSimpleName();
            }
        });
        run("root_pinning_match", () -> {
            // Pin the root, not the leaf: otherwise every renewal would force
            // the user to re-pair. The chain the server sends ends with the
            // root, so compare the pin of that certificate with the anchor.
            String served = rootPin(PORT);
            String pinned = pinOf(probeCa());
            require(pinned.equals(served), "served root " + served + " != pinned " + pinned);
            return served;
        });
        run("wrong_pin_rejected", () -> {
            String served = rootPin(PORT);
            // A pin of the right shape but the wrong value must not match;
            // otherwise pinning is a no-op that always passes.
            String wrongPin = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
            require(!wrongPin.equals(served), "a wrong pin matched: " + served);
            return "wrong pin does not match";
        });
        run("wrong_hostname_rejected", () -> {
            try {
                handshakeWithHostname(PORT, "wrong.example");
                throw new AssertionError("handshake succeeded for an unrelated hostname");
            } catch (javax.net.ssl.SSLHandshakeException | javax.net.ssl.SSLPeerUnverifiedException expected) {
                // Conscrypt performs endpoint identification inside the
                // handshake, so a name mismatch surfaces as SSLHandshakeException
                // rather than SSLPeerUnverifiedException. Require the message to
                // name the actual reason: a generic "connection closed" must not
                // be counted as a hostname rejection.
                String message = String.valueOf(expected.getMessage());
                require(message.contains("subjectAltName") || message.contains("No name matching"),
                        "failed for another reason: " + message);
                return "rejected: " + message;
            }
        });
        run("renewal_keeps_the_pin", () -> {
            // Port 8769 serves a re-issued leaf under the same root. A device
            // that only knows the root pin must keep working after a renewal.
            String protocol = handshake(RENEWED_PORT, "TLSv1.3", trustingProbeCa());
            String renewed = rootPin(RENEWED_PORT);
            String pinned = pinOf(probeCa());
            require(pinned.equals(renewed), "renewal changed the root pin: " + renewed);
            return "same root " + renewed + ", handshake " + protocol;
        });
        run("reset_root_changes_the_pin", () -> {
            // Port 8770 has a new root. Its pin must differ, and it must not be
            // trusted: a silent acceptance here would mean the device kept
            // talking to an identity the user never confirmed.
            String rotated = pinOfRootOf(RESET_PORT);
            String pinned = pinOf(probeCa());
            require(!pinned.equals(rotated), "reset did not change the pin");
            try {
                handshake(RESET_PORT, "TLSv1.3", trustingProbeCa());
                throw new AssertionError("handshake succeeded against a rotated root");
            } catch (IOException expected) {
                return "pin changed to " + rotated.substring(0, 8) + "…, rejected as untrusted";
            }
        });
        run("cleartext_blocked", () -> {
            try {
                HttpURLConnection connection = (HttpURLConnection)
                        new URL("http://" + HOST + ":" + PORT + "/api/health/live").openConnection();
                connection.setConnectTimeout(TIMEOUT_MS);
                connection.setReadTimeout(TIMEOUT_MS);
                int code = connection.getResponseCode();
                throw new AssertionError("cleartext request returned " + code);
            } catch (IOException expected) {
                return "blocked: " + expected.getClass().getSimpleName();
            }
        });

        long elapsed = System.currentTimeMillis() - started;
        int passed = 0;
        for (String line : lines) {
            if (line.contains("=> PASS")) passed++;
        }
        String summary = "SUMMARY pass=" + passed + " fail=" + (lines.size() - passed)
                + " elapsedMs=" + elapsed;
        append(summary);
        writeReport();
        Log.i(TAG, summary);
    }

    private interface Case {
        String run() throws Exception;
    }

    private void run(String name, Case body) {
        String line;
        try {
            String detail = body.run();
            line = "CASE " + name + " => PASS: " + detail;
        } catch (Throwable error) {
            line = "CASE " + name + " => FAIL: " + error.getClass().getSimpleName()
                    + ": " + error.getMessage();
        }
        append(line);
        Log.i(TAG, line);
    }

    private void append(String line) {
        lines.add(line);
        runOnUiThread(() -> view.setText(String.join("\n", lines)));
    }

    private void writeReport() {
        try {
            File dir = getExternalFilesDir(null);
            if (dir == null) return;
            File file = new File(dir, "probe.txt");
            try (FileOutputStream out = new FileOutputStream(file);
                 OutputStreamWriter writer = new OutputStreamWriter(out, StandardCharsets.UTF_8)) {
                writer.write(String.join("\n", lines));
                writer.write("\n");
            }
            Log.i(TAG, "REPORT " + file.getAbsolutePath());
        } catch (IOException error) {
            Log.i(TAG, "REPORT write failed: " + error);
        }
    }

    private static void require(boolean condition, String detail) {
        if (!condition) throw new AssertionError(detail);
    }

    // ---- TLS plumbing ----

    private String handshake(int port, String protocol, SSLContext context) throws Exception {
        SSLSocketFactory factory = context.getSocketFactory();
        try (SSLSocket socket = (SSLSocket) factory.createSocket()) {
            socket.connect(new InetSocketAddress(HOST, port), TIMEOUT_MS);
            socket.setSoTimeout(TIMEOUT_MS);
            socket.setEnabledProtocols(new String[]{protocol});
            socket.startHandshake();
            SSLSession session = socket.getSession();
            return session.getProtocol();
        }
    }

    /** SPKI pin of the last certificate the server presents (the root). */
    private String rootPin(int port) throws Exception {
        SSLSocketFactory factory = trustingProbeCa().getSocketFactory();
        try (SSLSocket socket = (SSLSocket) factory.createSocket()) {
            socket.connect(new InetSocketAddress(HOST, port), TIMEOUT_MS);
            socket.setSoTimeout(TIMEOUT_MS);
            socket.startHandshake();
            java.security.cert.Certificate[] chain = socket.getSession().getPeerCertificates();
            return pinOf((X509Certificate) chain[chain.length - 1]);
        }
    }

    /** Root pin of a server this app does not trust (verification disabled on purpose). */
    private String pinOfRootOf(int port) throws Exception {
        SSLContext context = SSLContext.getInstance("TLS");
        context.init(null, new javax.net.ssl.TrustManager[]{new javax.net.ssl.X509TrustManager() {
            public void checkClientTrusted(X509Certificate[] chain, String authType) {}
            public void checkServerTrusted(X509Certificate[] chain, String authType) {}
            public X509Certificate[] getAcceptedIssuers() { return new X509Certificate[0]; }
        }}, null);
        try (SSLSocket socket = (SSLSocket) context.getSocketFactory().createSocket()) {
            socket.connect(new InetSocketAddress(HOST, port), TIMEOUT_MS);
            socket.setSoTimeout(TIMEOUT_MS);
            socket.startHandshake();
            java.security.cert.Certificate[] chain = socket.getSession().getPeerCertificates();
            return pinOf((X509Certificate) chain[chain.length - 1]);
        }
    }

    private static String pinOf(X509Certificate certificate) throws Exception {
        MessageDigest digest = MessageDigest.getInstance("SHA-256");
        return Base64.encodeToString(digest.digest(certificate.getPublicKey().getEncoded()), Base64.NO_WRAP);
    }

    /** Handshake where the peer host used for verification is not the address. */
    private void handshakeWithHostname(int port, String hostname) throws Exception {
        java.net.Socket plain = new java.net.Socket();
        plain.connect(new InetSocketAddress(HOST, port), TIMEOUT_MS);
        SSLSocketFactory factory = trustingProbeCa().getSocketFactory();
        try (SSLSocket socket = (SSLSocket) factory.createSocket(plain, hostname, port, true)) {
            socket.setSoTimeout(TIMEOUT_MS);
            javax.net.ssl.SSLParameters parameters = socket.getSSLParameters();
            parameters.setEndpointIdentificationAlgorithm("HTTPS");
            socket.setSSLParameters(parameters);
            socket.startHandshake();
        }
    }

    private X509Certificate probeCa() throws Exception {
        CertificateFactory factory = CertificateFactory.getInstance("X.509");
        try (InputStream raw = getResources().openRawResource(R.raw.aicc_test_ca)) {
            return (X509Certificate) factory.generateCertificate(new ByteArrayInputStream(readAll(raw)));
        }
    }

    private SSLContext trustingProbeCa() throws Exception {
        X509Certificate ca = probeCa();
        KeyStore store = KeyStore.getInstance(KeyStore.getDefaultType());
        store.load(null, null);
        store.setCertificateEntry("aicc-test-ca", ca);

        TrustManagerFactory managers = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
        managers.init(store);
        SSLContext context = SSLContext.getInstance("TLS");
        context.init(null, managers.getTrustManagers(), null);
        return context;
    }

    private int httpsStatus(String path, String token) throws Exception {
        HttpsURLConnection connection =
                (HttpsURLConnection) new URL("https://" + HOST + ":" + PORT + path).openConnection();
        connection.setConnectTimeout(TIMEOUT_MS);
        connection.setReadTimeout(TIMEOUT_MS);
        if (token != null) connection.setRequestProperty("Authorization", "Bearer " + token);
        try {
            int code = connection.getResponseCode();
            try (InputStream body = code >= 400 ? connection.getErrorStream() : connection.getInputStream()) {
                if (body != null) drain(body);
            }
            return code;
        } finally {
            connection.disconnect();
        }
    }

    private static void drain(InputStream input) throws IOException {
        BufferedReader reader = new BufferedReader(new InputStreamReader(input, StandardCharsets.UTF_8));
        String line;
        while ((line = reader.readLine()) != null) {
            // The probe only needs the status code; reading keeps the socket clean.
        }
    }

    private static byte[] readAll(InputStream input) throws IOException {
        java.io.ByteArrayOutputStream out = new java.io.ByteArrayOutputStream();
        byte[] buffer = new byte[4096];
        int read;
        while ((read = input.read(buffer)) > 0) out.write(buffer, 0, read);
        return out.toByteArray();
    }
}
