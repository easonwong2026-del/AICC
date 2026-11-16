package com.aieink.pokedashboard;

import java.io.ByteArrayInputStream;
import java.io.InputStream;
import java.security.KeyStore;
import java.security.MessageDigest;
import java.security.PublicKey;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;

import javax.net.ssl.SSLContext;
import javax.net.ssl.TrustManager;
import javax.net.ssl.TrustManagerFactory;
import javax.net.ssl.X509TrustManager;

/**
 * TLS for the bridge: certificate validation plus a pin on the root's public key.
 *
 * The Mac serves a self-signed root, which no system store knows about, so the
 * anchor is established out of band: the user compares the SHA-256 fingerprint
 * on the Mac with the one on this screen, and only then does the app trust that
 * key. Two contexts follow from that:
 *
 * {@link #bootstrap} is used for exactly one request — the pairing POST — on a
 * connection that is *not* validated by PKIX (there is no anchor yet), but only
 * if the certificate presented carries the confirmed fingerprint. That is the
 * same trust decision, verified instead of assumed.
 *
 * {@link #anchored} is used for everything else: normal PKIX validation against
 * the root we stored at pairing time, plus a pin check on the root's SPKI so a
 * swapped-out certificate from another CA cannot be substituted later.
 *
 * There is no trust-all path, no hostname-verification switch, and no fallback
 * to cleartext anywhere in this class.
 */
final class BridgeTls {

    private BridgeTls() {}

    /** What the app remembers about a Mac it trusts. */
    static final class Identity {
        final String caPem;
        final String pin;

        Identity(String caPem, String pin) {
            this.caPem = caPem;
            this.pin = pin;
        }
    }

    static String pin(PublicKey key) {
        try {
            MessageDigest digest = MessageDigest.getInstance("SHA-256");
            return base64(digest.digest(key.getEncoded()));
        } catch (Exception error) {
            throw new IllegalStateException("SHA-256 unavailable", error);
        }
    }

    private static final char[] ALPHABET =
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/".toCharArray();

    /**
     * Standard base64 with padding, written out here on purpose: the Android
     * helper is a stub when these tests run on the JVM, and java.util.Base64
     * needs API 26 while the app still installs on 23.
     */
    static String base64(byte[] bytes) {
        StringBuilder out = new StringBuilder((bytes.length + 2) / 3 * 4);
        for (int index = 0; index < bytes.length; index += 3) {
            int remaining = bytes.length - index;
            int chunk = (bytes[index] & 0xFF) << 16;
            if (remaining > 1) chunk |= (bytes[index + 1] & 0xFF) << 8;
            if (remaining > 2) chunk |= bytes[index + 2] & 0xFF;
            out.append(ALPHABET[(chunk >> 18) & 0x3F]);
            out.append(ALPHABET[(chunk >> 12) & 0x3F]);
            out.append(remaining > 1 ? ALPHABET[(chunk >> 6) & 0x3F] : '=');
            out.append(remaining > 2 ? ALPHABET[chunk & 0x3F] : '=');
        }
        return out.toString();
    }

    /**
     * Trust exactly the key with this fingerprint, and nothing else.
     * Intended for the pairing request only.
     */
    static SSLContext bootstrap(String expectedPin) throws Exception {
        if (expectedPin == null || expectedPin.length() != 44) {
            throw new IllegalArgumentException("A fingerprint is required before pairing");
        }
        SSLContext context = SSLContext.getInstance("TLS");
        context.init(null, new TrustManager[]{new PinOnlyTrustManager(expectedPin)}, null);
        return context;
    }

    /**
     * Normal chain validation against the stored root, plus the pin. The pin is
     * checked on the root of the presented chain, which is what the user
     * confirmed — leaves may be re-issued without touching it.
     */
    static SSLContext anchored(Identity identity) throws Exception {
        if (identity == null || identity.caPem == null || identity.pin == null) {
            throw new IllegalStateException("No pinned identity stored");
        }
        CertificateFactory factory = CertificateFactory.getInstance("X.509");
        X509Certificate anchor;
        try (InputStream input = new ByteArrayInputStream(identity.caPem.getBytes("UTF-8"))) {
            anchor = (X509Certificate) factory.generateCertificate(input);
        }
        if (!identity.pin.equals(pin(anchor.getPublicKey()))) {
            throw new IllegalStateException("Stored fingerprint does not match the stored root");
        }
        KeyStore store = KeyStore.getInstance(KeyStore.getDefaultType());
        store.load(null, null);
        store.setCertificateEntry("aicc-bridge-root", anchor);
        TrustManagerFactory managers = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
        managers.init(store);

        X509TrustManager delegate = null;
        for (TrustManager manager : managers.getTrustManagers()) {
            if (manager instanceof X509TrustManager) delegate = (X509TrustManager) manager;
        }
        if (delegate == null) throw new IllegalStateException("No X509 trust manager");

        SSLContext context = SSLContext.getInstance("TLS");
        context.init(null, new TrustManager[]{new PinnedTrustManager(delegate, identity.pin)}, null);
        return context;
    }

    /** PKIX via the delegate, then require the chain's root to be the pinned key. */
    private static final class PinnedTrustManager implements X509TrustManager {
        private final X509TrustManager delegate;
        private final String expectedPin;

        PinnedTrustManager(X509TrustManager delegate, String expectedPin) {
            this.delegate = delegate;
            this.expectedPin = expectedPin;
        }

        @Override
        public void checkClientTrusted(X509Certificate[] chain, String authType) {
            // The bridge authenticates devices with a bearer token, not a
            // client certificate; there is nothing to accept here.
        }

        @Override
        public void checkServerTrusted(X509Certificate[] chain, String authType)
                throws java.security.cert.CertificateException {
            delegate.checkServerTrusted(chain, authType);
            verifyRoot(chain, expectedPin);
        }

        @Override
        public X509Certificate[] getAcceptedIssuers() {
            return delegate.getAcceptedIssuers();
        }
    }

    /** No PKIX: only the fingerprint matters, and it must match exactly. */
    private static final class PinOnlyTrustManager implements X509TrustManager {
        private final String expectedPin;

        PinOnlyTrustManager(String expectedPin) {
            this.expectedPin = expectedPin;
        }

        @Override
        public void checkClientTrusted(X509Certificate[] chain, String authType) {
        }

        @Override
        public void checkServerTrusted(X509Certificate[] chain, String authType)
                throws java.security.cert.CertificateException {
            verifyRoot(chain, expectedPin);
        }

        @Override
        public X509Certificate[] getAcceptedIssuers() {
            return new X509Certificate[0];
        }
    }

    /**
     * The pin is on the root, so walk to the last certificate the server sent.
     * A device that pinned the leaf would have to be re-paired on every renewal.
     */
    private static void verifyRoot(X509Certificate[] chain, String expectedPin)
            throws java.security.cert.CertificateException {
        if (chain == null || chain.length == 0) {
            throw new java.security.cert.CertificateException("Server presented no certificate");
        }
        String served = pin(chain[chain.length - 1].getPublicKey());
        if (!served.equals(expectedPin)) {
            throw new java.security.cert.CertificateException(
                    "Server identity changed: expected " + expectedPin + " but saw " + served);
        }
    }
}
