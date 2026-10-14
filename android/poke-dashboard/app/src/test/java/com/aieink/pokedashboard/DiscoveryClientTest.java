package com.aieink.pokedashboard;

import static org.junit.Assert.*;

import org.junit.Test;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

public final class DiscoveryClientTest {
    private static final String STATUS = "{\"updated_at\":\"now\",\"codex\":{\"available\":true}}";

    @Test public void changedIpSelectsOnlyHealthyServer() throws Exception {
        try (FakeServer mac = new FakeServer("mac", true)) {
            int oldPort = unusedPort();
            assertNull(DiscoveryClient.probe(new DiscoveryClient.Candidate(url(oldPort))));
            List<DiscoveryClient.Candidate> found = discovered(500, 0, reply(mac.port(), true));
            List<DiscoveryClient.Candidate> healthy = DiscoveryClient.verify(found);
            assertEquals(url(mac.port()), DiscoveryClient.choose(healthy, "mac").url);
        }
    }

    @Test public void twoHealthyServersRequireChoiceOrKnownHost() throws Exception {
        try (FakeServer windows = new FakeServer("windows", true);
             FakeServer mac = new FakeServer("mac", true)) {
            List<DiscoveryClient.Candidate> healthy = DiscoveryClient.verify(discovered(
                    500, 0, reply(windows.port(), true), reply(mac.port(), true)));
            assertEquals(2, healthy.size());
            assertNull(DiscoveryClient.choose(healthy, null));
            assertEquals(url(mac.port()), DiscoveryClient.choose(healthy, "mac").url);
        }
    }

    @Test public void deadFirstResponderDoesNotHideLaterMac() throws Exception {
        try (FakeServer mac = new FakeServer("mac", true)) {
            List<DiscoveryClient.Candidate> found = discovered(
                    500, 0, reply(unusedPort(), true), reply(mac.port(), true));
            assertEquals(2, found.size());
            List<DiscoveryClient.Candidate> healthy = DiscoveryClient.verify(found);
            assertEquals(1, healthy.size());
            assertEquals(url(mac.port()), DiscoveryClient.choose(healthy, null).url);
        }
    }

    @Test public void lostFirstBroadcastIsRetried() throws Exception {
        try (FakeServer mac = new FakeServer("mac", true)) {
            List<DiscoveryClient.Candidate> found = discovered(1900, 1, reply(mac.port(), true));
            assertEquals(1, found.size());
            assertEquals(url(mac.port()), DiscoveryClient.choose(DiscoveryClient.verify(found), null).url);
        }
    }

    @Test public void noServerLeavesNoCandidate() throws Exception {
        List<DiscoveryClient.Candidate> found = discovered(200, 99);
        assertTrue(found.isEmpty());
        assertNull(DiscoveryClient.choose(DiscoveryClient.verify(found), null));
    }

    @Test public void legacyDiscoveryAndHealthRemainCompatible() throws Exception {
        try (FakeServer old = new FakeServer("", true)) {
            List<DiscoveryClient.Candidate> healthy = DiscoveryClient.verify(discovered(
                    500, 0, reply(old.port(), false)));
            assertEquals(1, healthy.size());
            assertEquals(url(old.port()), healthy.get(0).url);
        }
    }

    @Test public void statusFailureRejectsHealthyLookingResponder() throws Exception {
        try (FakeServer broken = new FakeServer("broken", false)) {
            assertNull(DiscoveryClient.probe(new DiscoveryClient.Candidate(url(broken.port()))));
        }
    }

    @Test public void unpairedBridgeIsDiscoverableButCredentialNeverCrossesHosts() throws Exception {
        try (FakeServer bridge = new FakeServer("paired-mac", true, true)) {
            DiscoveryClient.Candidate waiting = DiscoveryClient.probe(new DiscoveryClient.Candidate(url(bridge.port())));
            assertNotNull(waiting);
            assertTrue(waiting.pairingRequired);
            assertEquals(0, bridge.statusRequests.get());
            DiscoveryClient.Candidate other = DiscoveryClient.probe(new DiscoveryClient.Candidate(url(bridge.port())),
                    "test-read-only-token", "other-mac");
            assertTrue(other.pairingRequired);
            assertEquals(0, bridge.statusRequests.get());
            DiscoveryClient.Candidate paired = DiscoveryClient.probe(new DiscoveryClient.Candidate(url(bridge.port())),
                    "test-read-only-token", "paired-mac");
            assertNotNull(paired.data);
            assertFalse(paired.pairingRequired);
            DiscoveryClient.Candidate revoked = DiscoveryClient.probe(new DiscoveryClient.Candidate(url(bridge.port())),
                    "old-token", "paired-mac");
            assertTrue(revoked.pairingRequired);
        }
    }

    private static String url(int port) { return "http://127.0.0.1:" + port; }

    @Test public void manualAddressRejectsCredentialsAndPublicDestinations() {
        assertEquals("http://192.168.1.20:8765", DiscoveryClient.normaliseBaseUrl("192.168.1.20"));
        assertEquals("http://10.0.0.3:9876", DiscoveryClient.normaliseBaseUrl("http://10.0.0.3:9876/api/status"));
        for (String invalid : new String[] { "http://user:key@192.168.1.20:8765", "http://example.com:8765",
                "http://8.8.8.8:8765", "http://192.168.1.20:0", "http://192.168.1.20:8765/?secret=key",
                "http://192.168.1.20:8765/private", "http://192.168.01.20:8765", "file:///tmp/key" }) {
            assertNull(DiscoveryClient.normaliseBaseUrl(invalid));
        }
    }

    private static int unusedPort() throws IOException {
        try (java.net.ServerSocket socket = new java.net.ServerSocket(0)) {
            return socket.getLocalPort();
        }
    }

    private static String reply(int port, boolean modern) {
        return "{\"name\":\"AICC Dashboard\",\"port\":" + port
                + (modern ? ",\"protocol\":\"aicc\"" : "") + "}";
    }

    private static List<DiscoveryClient.Candidate> discovered(
            long windowMs, int dropCount, String... replies) throws Exception {
        InetAddress loopback = InetAddress.getByName("127.0.0.1");
        try (DatagramSocket listener = new DatagramSocket(0, loopback)) {
            AtomicInteger requests = new AtomicInteger();
            Thread responder = new Thread(() -> {
                byte[] buffer = new byte[512];
                while (!listener.isClosed()) {
                    DatagramPacket request = new DatagramPacket(buffer, buffer.length);
                    try {
                        listener.receive(request);
                        if (requests.incrementAndGet() <= dropCount) continue;
                        for (String reply : replies) {
                            byte[] payload = reply.getBytes(StandardCharsets.UTF_8);
                            listener.send(new DatagramPacket(payload, payload.length,
                                    request.getAddress(), request.getPort()));
                        }
                    } catch (IOException error) {
                        break;
                    }
                }
            }, "fake-aicc-discovery");
            responder.setDaemon(true);
            responder.start();
            return DiscoveryClient.discover(listener.getLocalPort(), List.of(loopback), windowMs);
        }
    }

    private static final class FakeServer implements AutoCloseable {
        private final ServerSocket socket;
        private final Thread thread;

        final AtomicInteger statusRequests = new AtomicInteger();

        FakeServer(String hostId, boolean validStatus) throws IOException { this(hostId, validStatus, false); }

        FakeServer(String hostId, boolean validStatus, boolean pairing) throws IOException {
            socket = new ServerSocket(0, 0, InetAddress.getByName("127.0.0.1"));
            String identity = "{\"ok\":true,\"status\":\"live\",\"version\":\"2.8.0\""
                    + (hostId.isEmpty() ? "" : ",\"protocol\":\"aicc\",\"host_id\":\"" + hostId + "\"")
                    + ",\"pairing_required\":" + pairing + "}";
            thread = new Thread(() -> {
                while (!socket.isClosed()) {
                    try (Socket client = socket.accept()) {
                        BufferedReader input = new BufferedReader(new InputStreamReader(
                                client.getInputStream(), StandardCharsets.US_ASCII));
                        String request = input.readLine();
                        String header;
                        String authorization = null;
                        while ((header = input.readLine()) != null && !header.isEmpty()) {
                            if (header.startsWith("Authorization: ")) authorization = header.substring(15);
                        }
                        if (request == null) continue;
                        if (request.startsWith("GET /api/health/live ")) send(client, 200, identity);
                        else if (request.startsWith("GET /api/status ")) {
                            statusRequests.incrementAndGet();
                            if (pairing && !"Bearer test-read-only-token".equals(authorization)) send(client, 401, "{}");
                            else send(client, validStatus ? 200 : 503, validStatus ? STATUS : "{}");
                        } else send(client, 404, "{}");
                    } catch (IOException error) {
                        if (!socket.isClosed()) throw new RuntimeException(error);
                    }
                }
            }, "fake-aicc-http");
            thread.setDaemon(true);
            thread.start();
        }

        int port() { return socket.getLocalPort(); }
        @Override public void close() throws IOException {
            socket.close();
            try { thread.join(1000); } catch (InterruptedException error) { Thread.currentThread().interrupt(); }
        }

        private static void send(Socket client, int code, String body) throws IOException {
            byte[] data = body.getBytes(StandardCharsets.UTF_8);
            OutputStream output = client.getOutputStream();
            output.write(("HTTP/1.1 " + code + " " + (code == 200 ? "OK" : "Unavailable")
                    + "\r\nContent-Type: application/json\r\nContent-Length: " + data.length
                    + "\r\nConnection: close\r\n\r\n").getBytes(StandardCharsets.US_ASCII));
            output.write(data);
            output.flush();
        }
    }
}
