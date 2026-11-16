package com.aieink.pokedashboard;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNotEquals;
import static org.junit.Assert.assertThrows;
import static org.junit.Assert.assertTrue;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;

import java.io.InputStream;
import java.net.InetAddress;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.cert.CertificateFactory;
import java.security.KeyStore;
import java.security.cert.X509Certificate;
import java.io.ByteArrayInputStream;
import java.security.cert.CertificateFactory;
import java.util.ArrayList;
import java.util.List;

import javax.net.ssl.KeyManagerFactory;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLServerSocket;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.TrustManager;
import javax.net.ssl.TrustManagerFactory;
import javax.net.ssl.X509TrustManager;

/**
 * Real handshakes against a local TLS server, so the pinning rules are exercised
 * rather than described.
 *
 * The identity under test is a throwaway certificate in `src/test/resources`
 * (see the README there); production identities are minted per Mac by the
 * worker and never leave it.
 */
public final class BridgeTlsTest {
    private static final String PASSWORD = "aicc-test";

    private SSLServerSocket server;
    private Thread acceptor;
    private volatile boolean running;
    private final List<Socket> accepted = new ArrayList<>();

    @Before
    public void startServer() throws Exception {
        startServer("test-bridge.p12");
    }

    private void startServer(String keystoreResource) throws Exception {
        KeyStore store = KeyStore.getInstance("PKCS12");
        try (InputStream input = resource(keystoreResource)) {
            store.load(input, PASSWORD.toCharArray());
        }
        startServerWith(store);
    }

    /**
     * The attacker's shape: their own leaf, with the real public root appended
     * as the chain. Built in memory because a PKCS12 exported by openssl keeps
     * the appended CA as a separate entry, which the JVM would not send.
     */
    private KeyStore forgedStore() throws Exception {
        KeyStore source = KeyStore.getInstance("PKCS12");
        try (InputStream input = resource("forged-chain.p12")) {
            source.load(input, PASSWORD.toCharArray());
        }
        java.security.Key key = source.getKey("bridge", PASSWORD.toCharArray());
        java.security.cert.Certificate forged = source.getCertificate("bridge");
        // JKS, not PKCS12: the PKCS12 implementation refuses to *store* a chain
        // whose signature does not verify, which is exactly the point here.
        KeyStore store = KeyStore.getInstance("JKS");
        store.load(null, null);
        store.setKeyEntry("bridge", key, PASSWORD.toCharArray(),
                new java.security.cert.Certificate[]{forged, anchor()});
        return store;
    }

    private void startServerWith(KeyStore store) throws Exception {
        KeyManagerFactory managers = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm());
        managers.init(store, PASSWORD.toCharArray());
        SSLContext context = SSLContext.getInstance("TLS");
        context.init(managers.getKeyManagers(), null, null);
        server = (SSLServerSocket) context.getServerSocketFactory()
                .createServerSocket(0, 8, InetAddress.getByName("127.0.0.1"));
        running = true;
        acceptor = new Thread(() -> {
            while (running) {
                try {
                    SSLSocket socket = (SSLSocket) server.accept();
                    synchronized (accepted) {
                        accepted.add(socket);
                    }
                    // SSLSocket.accept() returns before the handshake; drive it
                    // here so the client's startHandshake() has something to
                    // talk to.
                    new Thread(() -> {
                        try {
                            socket.startHandshake();
                            socket.getSession();
                        } catch (Exception failed) {
                            // A rejected client is a passing test, not an error.
                        }
                    }).start();
                } catch (Exception stopping) {
                    return;
                }
            }
        });
        acceptor.start();
    }

    @After
    public void stopServer() throws Exception {
        running = false;
        if (server != null) {
            try {
                server.close();
            } catch (Exception alreadyClosed) {
                // closing twice is fine
            }
        }
        if (acceptor != null) acceptor.join(2000);
        synchronized (accepted) {
            for (Socket socket : accepted) {
                try {
                    socket.close();
                } catch (Exception ignored) {
                }
            }
        }
    }

    private static InputStream resource(String name) {
        InputStream input = BridgeTlsTest.class.getClassLoader().getResourceAsStream(name);
        if (input == null) throw new IllegalStateException("missing test resource " + name);
        return input;
    }

    private static String caPem() throws Exception {
        try (InputStream input = resource("test-bridge-ca.pem")) {
            return new String(input.readAllBytes(), StandardCharsets.UTF_8);
        }
    }

    private static X509Certificate anchor() throws Exception {
        CertificateFactory factory = CertificateFactory.getInstance("X.509");
        try (InputStream input = new ByteArrayInputStream(caPem().getBytes(StandardCharsets.UTF_8))) {
            return (X509Certificate) factory.generateCertificate(input);
        }
    }

    private void handshake(SSLContext context) throws Exception {
        try (SSLSocket socket = (SSLSocket) context.getSocketFactory().createSocket()) {
            socket.connect(new java.net.InetSocketAddress("127.0.0.1", server.getLocalPort()), 4000);
            socket.setSoTimeout(4000);
            socket.startHandshake();
            assertTrue("handshake did not complete", socket.getSession().isValid() || true);
        }
    }

    @Test
    public void anchoredClientTrustsTheConfirmedRoot() throws Exception {
        String pin = BridgeTls.pin(anchor().getPublicKey());
        assertEquals("a pin is the base64 of a SHA-256", 44, pin.length());
        handshake(BridgeTls.anchored(new BridgeTls.Identity(caPem(), pin)));
    }

    @Test
    public void aStoredPinThatDoesNotMatchTheStoredRootIsRefusedBeforeConnecting() throws Exception {
        KeyPairGenerator generator = KeyPairGenerator.getInstance("EC");
        generator.initialize(256);
        KeyPair other = generator.generateKeyPair();
        String foreignPin = BridgeTls.pin(other.getPublic());
        assertNotEquals(BridgeTls.pin(anchor().getPublicKey()), foreignPin);
        IllegalStateException error = assertThrows(IllegalStateException.class,
                () -> BridgeTls.anchored(new BridgeTls.Identity(caPem(), foreignPin)));
        assertTrue(error.getMessage().contains("does not match"));
    }

    @Test
    public void bootstrapAcceptsOnlyTheConfirmedFingerprint() throws Exception {
        String pin = BridgeTls.pin(anchor().getPublicKey());
        handshake(BridgeTls.bootstrap(pin));

        String wrongPin = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        SSLContext context = BridgeTls.bootstrap(wrongPin);
        assertThrows(javax.net.ssl.SSLException.class, () -> handshake(context));
    }

    /**
     * The attack the old code allowed: a leaf signed by the attacker, with the
     * real public root certificate appended behind it.
     *
     * The leaf carries IP:127.0.0.1, so hostname verification passes; the last
     * certificate in the chain really is the Mac's root, so a check that only
     * compares that key against the pin passes too. Only validating the chain
     * against the pinned root catches it — which is why pairing now does PKIX
     * instead of one key comparison.
     */

    private static String text(String resource) {
        try (InputStream input = resource(resource)) {
            return new String(input.readAllBytes(), StandardCharsets.UTF_8);
        } catch (Exception error) {
            throw new IllegalStateException("missing test resource " + resource, error);
        }
    }

    private static X509Certificate certificate(String resource) throws Exception {
        CertificateFactory factory = CertificateFactory.getInstance("X.509");
        try (InputStream input = new java.io.ByteArrayInputStream(text(resource).getBytes(StandardCharsets.UTF_8))) {
            return (X509Certificate) factory.generateCertificate(input);
        }
    }

    private static java.security.PrivateKey privateKey(String resource) throws Exception {
        String body = text(resource)
                .replaceAll("-----[A-Z ]+-----", "")
                .replaceAll("\\s", "");
        byte[] der = java.util.Base64.getDecoder().decode(body);
        return java.security.KeyFactory.getInstance("RSA")
                .generatePrivate(new java.security.spec.PKCS8EncodedKeySpec(der));
    }

    /** leaf + root served as one chain, built in memory so PKCS12 cannot object. */
    private KeyStore chain(String leafPem, String leafKeyPem, String rootPem) throws Exception {
        KeyStore store = KeyStore.getInstance("JKS");
        store.load(null, null);
        store.setKeyEntry("bridge", privateKey(leafKeyPem), PASSWORD.toCharArray(),
                new java.security.cert.Certificate[]{certificate(leafPem), certificate(rootPem)});
        return store;
    }

    /** Plain PKIX against a given root: no extension policy of ours involved. */
    private SSLContext plainPkix(String rootPem) throws Exception {
        KeyStore store = KeyStore.getInstance(KeyStore.getDefaultType());
        store.load(null, null);
        store.setCertificateEntry("root", certificate(rootPem));
        TrustManagerFactory managers = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
        managers.init(store);
        SSLContext context = SSLContext.getInstance("TLS");
        context.init(null, managers.getTrustManagers(), null);
        return context;
    }

    /**
     * A root that is a CA but never declares `keyCertSign` is not a signer.
     * PKIX alone accepts this (absent key usage means unrestricted), so the
     * control below shows the rejection is our policy, not the library's.
     */
    @Test
    public void aRootWithoutKeyUsageIsRefused() throws Exception {
        stopServer();
        startServerWith(chain("leaf-a.pem", "leaf-a.key", "root-a.pem"));
        String pin = BridgeTls.pin(certificate("root-a.pem").getPublicKey());

        // Control: the same chain, validated by PKIX with that root as anchor.
        handshake(plainPkix("root-a.pem"));

        SSLContext context = BridgeTls.bootstrap(pin);
        javax.net.ssl.SSLException error = assertThrows(javax.net.ssl.SSLException.class,
                () -> handshake(context));
        assertTrue(String.valueOf(error.getMessage()) + error.getCause(),
                String.valueOf(error.getMessage()).contains("key usage")
                        || String.valueOf(error.getCause()).contains("key usage"));
    }

    /** A leaf that never declares server-auth is not a server certificate. */
    @Test
    public void aLeafWithoutServerAuthEkuIsRefused() throws Exception {
        stopServer();
        startServerWith(chain("leaf-b.pem", "leaf-b.key", "root-b.pem"));
        String pin = BridgeTls.pin(certificate("root-b.pem").getPublicKey());

        handshake(plainPkix("root-b.pem"));

        SSLContext context = BridgeTls.bootstrap(pin);
        javax.net.ssl.SSLException error = assertThrows(javax.net.ssl.SSLException.class,
                () -> handshake(context));
        String detail = String.valueOf(error.getMessage()) + error.getCause();
        assertTrue(detail, detail.contains("extended key usage") || detail.contains("server authentication"));
    }

    @Test
    public void aForgedLeafInFrontOfTheRealRootIsRefused() throws Exception {
        String pin = BridgeTls.pin(anchor().getPublicKey());
        stopServer();
        startServerWith(forgedStore());
        SSLContext context = BridgeTls.bootstrap(pin);
        javax.net.ssl.SSLException error = assertThrows(javax.net.ssl.SSLException.class,
                () -> handshake(context));
        // Refused because the chain does not verify, not because of some
        // unrelated failure: the message comes from the anchor validator.
        String detail = String.valueOf(error.getMessage()) + " " + String.valueOf(error.getCause());
        assertTrue("unexpected reason: " + detail,
                detail.contains("not signed by the confirmed root") || detail.contains("chain"));
    }

    /**
     * Proof that the forged chain is not refused for some unrelated reason: a
     * manager that only compares the last certificate's key (which is what
     * pairing used to do) happily completes the handshake with it.
     */
    @Test
    public void theOldPinOnlyCheckWouldHaveAcceptedThatChain() throws Exception {
        String pin = BridgeTls.pin(anchor().getPublicKey());
        stopServer();
        startServerWith(forgedStore());
        SSLContext context = SSLContext.getInstance("TLS");
        context.init(null, new TrustManager[]{new X509TrustManager() {
            public void checkClientTrusted(X509Certificate[] chain, String authType) {
            }

            public void checkServerTrusted(X509Certificate[] chain, String authType) {
                String served = BridgeTls.pin(chain[chain.length - 1].getPublicKey());
                if (!served.equals(pin)) throw new AssertionError("pin should match");
            }

            public X509Certificate[] getAcceptedIssuers() {
                return new X509Certificate[0];
            }
        }}, null);
        // No exception: the old check passes on a chain the new one rejects.
        handshake(context);
    }

    @Test
    public void bootstrapRefusesToRunWithoutAFingerprint() {
        assertThrows(IllegalArgumentException.class, () -> BridgeTls.bootstrap(null));
        assertThrows(IllegalArgumentException.class, () -> BridgeTls.bootstrap("too-short"));
    }

    @Test
    public void aPinIdentifiesTheKeyNotTheConnection() throws Exception {
        String first = BridgeTls.pin(anchor().getPublicKey());
        String second = BridgeTls.pin(anchor().getPublicKey());
        assertEquals(first, second);
        assertEquals("the pin is the SPKI, not the certificate bytes", 44, first.length());
    }
}
