package com.aieink.pokedashboard;

import android.util.Log;

import org.json.JSONObject;

import java.io.BufferedReader;
import java.io.InputStreamReader;
import java.net.HttpURLConnection;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.Inet4Address;
import java.net.InetAddress;
import java.net.InterfaceAddress;
import java.net.NetworkInterface;
import java.net.SocketTimeoutException;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Enumeration;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;

final class DiscoveryClient {
    private static final int DISCOVERY_PORT = 8766;
    private static final byte[] REQUEST = "AI_EINK_DISCOVER".getBytes(StandardCharsets.UTF_8);
    private static final String TAG = "AICC Discovery";

    static final class Candidate {
        final String url;
        String name;
        String hostname;
        String platform;
        String hostId;
        String statusBody;
        DashboardData data;
        boolean stale;

        Candidate(String url) { this.url = url; }
    }

    private DiscoveryClient() {}

    static List<Candidate> discover() {
        try {
            return discover(DISCOVERY_PORT, broadcastAddresses(), 3500);
        } catch (Exception error) {
            Log.w(TAG, "Discovery unavailable", error);
            return new ArrayList<>();
        }
    }

    static List<Candidate> discover(int port, List<InetAddress> addresses, long windowMs) {
        LinkedHashMap<String, Candidate> candidates = new LinkedHashMap<>();
        try (DatagramSocket socket = new DatagramSocket()) {
            socket.setBroadcast(true);
            long deadline = elapsedMs() + windowMs;
            long nextSend = 0;
            int sends = 0;
            Log.i(TAG, "Discovery started");
            while (elapsedMs() < deadline) {
                long now = elapsedMs();
                if (sends < 3 && now >= nextSend) {
                    for (InetAddress address : addresses) {
                        try {
                            socket.send(new DatagramPacket(REQUEST, REQUEST.length, address, port));
                        } catch (Exception error) {
                            Log.w(TAG, "Broadcast failed to " + address.getHostAddress(), error);
                        }
                    }
                    sends++;
                    nextSend = now + 800;
                }
                long until = Math.min(deadline, sends < 3 ? nextSend : deadline) - elapsedMs();
                socket.setSoTimeout((int) Math.max(1, until));
                byte[] buffer = new byte[512];
                DatagramPacket response = new DatagramPacket(buffer, buffer.length);
                try {
                    socket.receive(response);
                    Candidate candidate = parse(response);
                    if (candidate != null && candidates.size() < 16) {
                        candidates.putIfAbsent(candidate.url, candidate);
                        Log.i(TAG, "Candidate found: " + candidate.url);
                    }
                } catch (SocketTimeoutException ignored) {
                    // Send the next broadcast or finish the discovery window.
                } catch (Exception error) {
                    Log.w(TAG, "Candidate rejected", error);
                }
            }
        } catch (Exception error) {
            Log.w(TAG, "Discovery unavailable", error);
        }
        return new ArrayList<>(candidates.values());
    }

    static List<Candidate> findServers() {
        return verify(discover());
    }

    static List<Candidate> verify(List<Candidate> candidates) {
        List<Candidate> healthy = new ArrayList<>();
        for (Candidate candidate : candidates) {
            Candidate checked = probe(candidate);
            if (checked != null) healthy.add(checked);
        }
        return healthy;
    }

    static Candidate probe(Candidate candidate) {
        try {
            JSONObject health = new JSONObject(request(candidate.url + "/api/health/live"));
            String protocol = health.optString("protocol", "");
            if (!health.optBoolean("ok") || !"live".equals(health.optString("status"))
                    || health.optString("version", "").isEmpty()
                    || (!protocol.isEmpty() && !"aicc".equals(protocol))) {
                throw new IllegalStateException("Not an AICC live server");
            }
            String body = request(candidate.url + "/api/status");
            JSONObject status = new JSONObject(body);
            if (status.optJSONObject("codex") == null
                    && status.optJSONObject("google") == null
                    && status.optJSONObject("workbuddy") == null) {
                throw new IllegalStateException("Not an AICC status response");
            }
            candidate.data = DashboardData.parse(body);
            candidate.statusBody = body;
            candidate.hostId = health.optString("host_id", "");
            candidate.hostname = health.optString("hostname", candidate.hostname == null ? "" : candidate.hostname);
            candidate.platform = health.optString("platform", candidate.platform == null ? "" : candidate.platform);
            JSONObject codex = status.optJSONObject("codex");
            JSONObject google = status.optJSONObject("google");
            JSONObject deepseek = status.optJSONObject("deepseek");
            JSONObject workbuddy = status.optJSONObject("workbuddy");
            candidate.stale = (codex != null && codex.optBoolean("stale"))
                    || (google != null && google.optBoolean("stale"))
                    || (deepseek != null && deepseek.optBoolean("stale"))
                    || (workbuddy != null && workbuddy.optBoolean("balance_stale"));
            Log.i(TAG, "Candidate verified: " + candidate.url);
            return candidate;
        } catch (Exception error) {
            Log.w(TAG, "Candidate rejected: " + candidate.url + " (" + error.getClass().getSimpleName() + ")");
            return null;
        }
    }

    private static String request(String endpoint) throws Exception {
        HttpURLConnection connection = (HttpURLConnection) new URL(endpoint).openConnection();
        connection.setConnectTimeout(5000);
        connection.setReadTimeout(7000);
        connection.setUseCaches(false);
        connection.setRequestProperty("Accept", "application/json");
        try {
            if (connection.getResponseCode() != 200) throw new IllegalStateException("HTTP " + connection.getResponseCode());
            StringBuilder body = new StringBuilder();
            try (BufferedReader reader = new BufferedReader(new InputStreamReader(
                    connection.getInputStream(), StandardCharsets.UTF_8))) {
                String line;
                while ((line = reader.readLine()) != null) body.append(line);
            }
            return body.toString();
        } finally {
            connection.disconnect();
        }
    }

    private static Candidate parse(DatagramPacket response) throws Exception {
        if (!(response.getAddress() instanceof Inet4Address)) return null;
        JSONObject payload = new JSONObject(new String(
                response.getData(), 0, response.getLength(), StandardCharsets.UTF_8));
        String protocol = payload.optString("protocol", "");
        if (!"aicc".equals(protocol)
                && !(protocol.isEmpty() && "AICC Dashboard".equals(payload.optString("name")))) {
            return null;
        }
        int port = payload.optInt("port", 8765);
        if (port < 1 || port > 65535) return null;
        Candidate candidate = new Candidate("http://" + response.getAddress().getHostAddress() + ":" + port);
        candidate.name = payload.optString("name", "AICC Dashboard");
        candidate.hostname = payload.optString("hostname", "");
        candidate.platform = payload.optString("platform", "");
        return candidate;
    }

    private static List<InetAddress> broadcastAddresses() throws Exception {
        LinkedHashSet<InetAddress> addresses = new LinkedHashSet<>();
        addresses.add(InetAddress.getByName("255.255.255.255"));
        try {
            Enumeration<NetworkInterface> interfaces = NetworkInterface.getNetworkInterfaces();
            while (interfaces != null && interfaces.hasMoreElements()) {
                NetworkInterface current = interfaces.nextElement();
                if (!current.isUp() || current.isLoopback()) continue;
                for (InterfaceAddress address : current.getInterfaceAddresses()) {
                    if (address.getBroadcast() != null) addresses.add(address.getBroadcast());
                }
            }
        } catch (Exception error) {
            Log.w(TAG, "Directed broadcast unavailable; trying limited broadcast", error);
        }
        return new ArrayList<>(addresses);
    }

    static Candidate choose(List<Candidate> verified, String preferredHostId) {
        if (preferredHostId != null && !preferredHostId.isEmpty()) {
            Candidate match = null;
            for (Candidate candidate : verified) {
                if (!preferredHostId.equals(candidate.hostId)) continue;
                if (match != null) return null;
                match = candidate;
            }
            if (match != null) return match;
        }
        return verified.size() == 1 ? verified.get(0) : null;
    }

    private static long elapsedMs() { return System.nanoTime() / 1_000_000L; }
}
