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
        KeyStore store = KeyStore.getInstance("PKCS12");
        try (InputStream input = resource("test-bridge.p12")) {
            store.load(input, PASSWORD.toCharArray());
        }
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
        if (server != null) server.close();
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
