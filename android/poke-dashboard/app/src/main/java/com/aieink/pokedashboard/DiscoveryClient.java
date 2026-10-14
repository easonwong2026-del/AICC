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
        boolean pairingRequired;

        Candidate(String url) { this.url = url; }
    }

    private DiscoveryClient() {}

    static String normaliseBaseUrl(String value) {
        if (value == null || value.trim().isEmpty()) return null;
        String input = value.trim();
        if (!input.contains("://")) input = "http://" + input;
        try {
            URL url = new URL(input);
            if ((!"http".equals(url.getProtocol()) && !"https".equals(url.getProtocol()))
                    || url.getUserInfo() != null || url.getQuery() != null || url.getRef() != null
                    || !(url.getPath().isEmpty() || "/".equals(url.getPath()) || "/api/status".equals(url.getPath()))) return null;
            String[] parts = url.getHost().split("\\.");
            if (parts.length != 4) return null;
            int[] octets = new int[4];
            for (int index = 0; index < 4; index++) {
                if (!parts[index].matches("0|[1-9][0-9]{0,2}")) return null;
                octets[index] = Integer.parseInt(parts[index]);
                if (octets[index] > 255) return null;
            }
            boolean local = octets[0] == 10 || octets[0] == 127
                    || (octets[0] == 192 && octets[1] == 168)
                    || (octets[0] == 172 && octets[1] >= 16 && octets[1] <= 31)
                    || (octets[0] == 169 && octets[1] == 254);
            int port = url.getPort() == -1 ? 8765 : url.getPort();
            if (!local || port < 1 || port > 65535) return null;
            return url.getProtocol() + "://" + url.getHost() + ":" + port;
        } catch (Exception error) { return null; }
    }

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
        return probe(candidate, null, null);
    }

    static Candidate probe(Candidate candidate, String token, String pairedHostId) {
        try {
            JSONObject health = new JSONObject(request(candidate.url + "/api/health/live"));
            String protocol = health.optString("protocol", "");
            if (!health.optBoolean("ok") || !"live".equals(health.optString("status"))
                    || health.optString("version", "").isEmpty()
                    || (!protocol.isEmpty() && !"aicc".equals(protocol))) {
                throw new IllegalStateException("Not an AICC live server");
            }
            candidate.hostId = health.optString("host_id", "");
            candidate.hostname = health.optString("hostname", candidate.hostname == null ? "" : candidate.hostname);
            candidate.platform = health.optString("platform", candidate.platform == null ? "" : candidate.platform);
            boolean paired = token != null && pairedHostId != null && pairedHostId.equals(candidate.hostId);
            if (health.optBoolean("pairing_required") && !paired) {
                candidate.pairingRequired = true;
                return candidate;
            }
            String body;
            try { body = request(candidate.url + "/api/status", paired ? token : null, null); }
            catch (Unauthorized error) {
                candidate.pairingRequired = true;
                return candidate;
            }
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
        return request(endpoint, null, null);
    }

    static JSONObject pair(String base, String code) throws Exception {
        if (!base.equals(normaliseBaseUrl(base))) throw new IllegalArgumentException("Local address required");
        JSONObject document = new JSONObject();
        document.put("code", code);
        return new JSONObject(request(base + "/api/pair", null, document.toString()));
    }

    private static final class Unauthorized extends Exception {}

    private static String request(String endpoint, String token, String payload) throws Exception {
        HttpURLConnection connection = (HttpURLConnection) new URL(endpoint).openConnection();
        connection.setConnectTimeout(5000);
        connection.setReadTimeout(7000);
        connection.setUseCaches(false);
        connection.setInstanceFollowRedirects(false);
        connection.setRequestProperty("Accept", "application/json");
        if (token != null) connection.setRequestProperty("Authorization", "Bearer " + token);
        try {
            if (payload != null) {
                connection.setRequestMethod("POST");
                connection.setRequestProperty("Content-Type", "application/json");
                connection.setDoOutput(true);
                try (java.io.OutputStream output = connection.getOutputStream()) {
                    output.write(payload.getBytes(StandardCharsets.UTF_8));
                }
            }
            if (connection.getResponseCode() == 401) throw new Unauthorized();
            if (connection.getResponseCode() != 200) throw new IllegalStateException("HTTP " + connection.getResponseCode());
            StringBuilder body = new StringBuilder();
            try (BufferedReader reader = new BufferedReader(new InputStreamReader(
                    connection.getInputStream(), StandardCharsets.UTF_8))) {
                String line;
                while ((line = reader.readLine()) != null) {
                    body.append(line);
                    if (body.length() > 1024 * 1024) throw new IllegalStateException("Response too large");
                }
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
