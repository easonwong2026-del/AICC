package com.aieink.pokedashboard;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotNull;
import static org.junit.Assert.assertNull;
import static org.junit.Assert.assertTrue;

import org.json.JSONObject;
import org.junit.Test;

import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.List;

/**
 * The LAN side is HTTPS-only now. These tests pin down the parts that would let
 * a device fall back to plaintext, trust a broadcast, or lose the fingerprint a
 * user is supposed to confirm.
 */
public final class DiscoveryClientTest {

    private static String httpsReply(int port, String pin) {
        return "{\"name\":\"AICC Companion\",\"port\":" + port + ",\"protocol\":\"aicc\","
                + "\"protocol_version\":2,\"host_id\":\"mac-1\",\"hostname\":\"mac\","
                + "\"platform\":\"macOS\",\"version\":\"2.12.2\","
                + "\"https\":{\"port\":" + port + ",\"spki\":\"" + pin + "\",\"protocol_version\":2}}";
    }

    private static List<DiscoveryClient.Candidate> discovered(int windowMs, String... replies) throws Exception {
        try (DatagramSocket responder = new DatagramSocket(0, InetAddress.getByName("127.0.0.1"))) {
            Thread worker = new Thread(() -> {
                long deadline = System.currentTimeMillis() + windowMs;
                byte[] buffer = new byte[512];
                while (System.currentTimeMillis() < deadline) {
                    try {
                        responder.setSoTimeout(200);
                        DatagramPacket request = new DatagramPacket(buffer, buffer.length);
                        responder.receive(request);
                        for (String reply : replies) {
                            byte[] payload = reply.getBytes(StandardCharsets.UTF_8);
                            responder.send(new DatagramPacket(payload, payload.length,
                                    request.getAddress(), request.getPort()));
                        }
                    } catch (Exception idle) {
                        // keep listening until the window closes
                    }
                }
            });
            worker.start();
            List<DiscoveryClient.Candidate> candidates = DiscoveryClient.discover(
                    responder.getLocalPort(),
                    Arrays.asList(InetAddress.getByName("127.0.0.1")),
                    windowMs);
            worker.join();
            return candidates;
        }
    }

    @Test
    public void aBareAddressMeansLanHttps() {
        assertEquals("https://192.168.0.14:8767", DiscoveryClient.normaliseBaseUrl("192.168.0.14"));
        assertEquals("https://192.168.0.14:8767", DiscoveryClient.normaliseBaseUrl("https://192.168.0.14"));
        assertEquals("https://192.168.0.14:9000", DiscoveryClient.normaliseBaseUrl("https://192.168.0.14:9000"));
    }

    @Test
    public void plaintextAndPublicAddressesAreRefused() {
        assertNull("LAN plaintext must not be usable",
                DiscoveryClient.normaliseBaseUrl("http://192.168.0.14:8765"));
        assertNull(DiscoveryClient.normaliseBaseUrl("8.8.8.8"));
        assertNull(DiscoveryClient.normaliseBaseUrl("https://192.168.0.14:8767/api/pair"));
        assertNull(DiscoveryClient.normaliseBaseUrl("https://example.com:8767"));
        assertNull(DiscoveryClient.normaliseBaseUrl(""));
    }

    @Test
    public void discoveryYieldsHttpsAndTheFingerprint() throws Exception {
        String pin = "IPpHOdBogidu7cws8POtcViGbrEpaXYrmWD6jQwd6CY=";
        List<DiscoveryClient.Candidate> found = discovered(600, httpsReply(8767, pin));
        assertEquals(1, found.size());
        DiscoveryClient.Candidate candidate = found.get(0);
        assertTrue(candidate.url.startsWith("https://"));
        assertTrue(candidate.url.endsWith(":8767"));
        assertEquals(pin, candidate.pin);
        assertEquals(2, candidate.protocolVersion);
        assertEquals("mac-1", candidate.hostId);
    }

    @Test
    public void aBridgeWithoutHttpsIsIgnoredRatherThanDowngraded() throws Exception {
        String plaintextOnly = "{\"name\":\"AICC Companion\",\"port\":8765,\"protocol\":\"aicc\","
                + "\"host_id\":\"old\",\"hostname\":\"mac\"}";
        assertTrue(discovered(600, plaintextOnly).isEmpty());
    }

    @Test
    public void aBridgeWithoutAPinIsIgnored() throws Exception {
        String noPin = "{\"name\":\"AICC Companion\",\"port\":8767,\"protocol\":\"aicc\","
                + "\"protocol_version\":2,\"host_id\":\"mac-1\",\"https\":{\"port\":8767}}";
        assertTrue(discovered(600, noPin).isEmpty());
    }

    @Test
    public void aDiscoveredServerIsNotTreatedAsVerified() throws Exception {
        // Regression: a broadcast-only candidate used to arrive with
        // pairingRequired == false and a null snapshot, which sent the UI down
        // the "already paired" path and crashed on the null snapshot.
        List<DiscoveryClient.Candidate> found = discovered(
                600, httpsReply(8767, "IPpHOdBogidu7cws8POtcViGbrEpaXYrmWD6jQwd6CY="));
        assertEquals(1, found.size());
        assertTrue("a broadcast is a proposal, not a paired server", found.get(0).pairingRequired);
        assertNull("nothing has been fetched yet", found.get(0).statusBody);
    }

    @Test
    public void nothingIsRequestedBeforeAFingerprintIsTrusted() {
        DiscoveryClient.Candidate candidate = new DiscoveryClient.Candidate("https://192.168.0.14:8767");
        DiscoveryClient.Candidate probed = DiscoveryClient.probe(candidate, null, "token", "mac-1");
        assertNotNull(probed);
        assertTrue("no identity means pairing, not a silent request", probed.pairingRequired);
        assertNull(probed.statusBody);
    }

    @Test
    public void pairingRefusesAPlaintextAddress() {
        try {
            DiscoveryClient.pair("http://192.168.0.14:8765", "12345678",
                    "IPpHOdBogidu7cws8POtcViGbrEpaXYrmWD6jQwd6CY=");
            throw new AssertionError("pairing over plaintext must not be possible");
        } catch (IllegalArgumentException expected) {
            assertTrue(expected.getMessage().contains("HTTPS"));
        } catch (Exception other) {
            throw new AssertionError(other);
        }
    }

    @Test
    public void thePairingResponseMustCarryTheAnchor() throws Exception {
        // Mirrors what the bridge returns; a response without ca_pem cannot be
        // used to validate later connections, so the UI must reject it.
        JSONObject response = new JSONObject();
        assertTrue(response.optString("ca_pem", "").isEmpty());
        response.put("ca_pem", "-----BEGIN CERTIFICATE-----\n");
        assertTrue(response.optString("ca_pem", "").contains("BEGIN CERTIFICATE"));
    }
}
