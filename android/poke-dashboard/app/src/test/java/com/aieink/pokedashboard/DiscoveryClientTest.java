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

    private static String url(int port) { return "http://127.0.0.1:" + port; }

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

        FakeServer(String hostId, boolean validStatus) throws IOException {
            socket = new ServerSocket(0, 0, InetAddress.getByName("127.0.0.1"));
            String identity = "{\"ok\":true,\"status\":\"live\",\"version\":\"2.8.0\""
                    + (hostId.isEmpty() ? "" : ",\"protocol\":\"aicc\",\"host_id\":\"" + hostId + "\"") + "}";
            thread = new Thread(() -> {
                while (!socket.isClosed()) {
                    try (Socket client = socket.accept()) {
                        BufferedReader input = new BufferedReader(new InputStreamReader(
                                client.getInputStream(), StandardCharsets.US_ASCII));
                        String request = input.readLine();
                        String header;
                        while ((header = input.readLine()) != null && !header.isEmpty()) { }
                        if (request == null) continue;
                        if (request.startsWith("GET /api/health/live ")) send(client, 200, identity);
                        else if (request.startsWith("GET /api/status ")) {
                            send(client, validStatus ? 200 : 503, validStatus ? STATUS : "{}");
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
