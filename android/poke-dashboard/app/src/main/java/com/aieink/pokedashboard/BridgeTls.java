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
 * TLS for the bridge: full chain validation plus a pin on the root's public key.
 *
 * The Mac serves a self-signed root, which no system store knows about, so the
 * anchor is established out of band: the user compares the SHA-256 fingerprint
 * on the Mac with the one on this screen, and only then does the app trust that
 * key. Two contexts follow from that:
 *
 * {@link #bootstrap} is used for exactly one request — the pairing POST — when
 * the anchor itself is what we are about to learn. The certificate the server
 * presents as its root must carry the confirmed fingerprint, and then it is
 * used as a real trust anchor: the whole chain is validated against it (leaf
 * signature, validity, CA bit, key usage, server-auth EKU). Comparing the last
 * certificate's key alone would be a bypass — an attacker could append the real
 * public root certificate to a leaf signed by their own key.
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
        context.init(null, new TrustManager[]{new PinnedAnchorTrustManager(expectedPin)}, null);
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

    /**
     * Pairing-time trust: the confirmed fingerprint identifies the anchor, and
     * the anchor then has to actually validate the chain.
     *
     * Checking only that the last certificate's key matches the pin (which this
     * used to do) accepts any leaf at all — forged leaf in front, real public
     * root behind it, pin matches, handshake succeeds. So the pinned root is
     * loaded as a trust anchor and the platform's PKIX validation runs against
     * it, with the CA and key-usage checks made explicit.
     */
    private static final class PinnedAnchorTrustManager implements X509TrustManager {
        private static final String SERVER_AUTH = "1.3.6.1.5.5.7.3.1";
        private final String expectedPin;

        PinnedAnchorTrustManager(String expectedPin) {
            this.expectedPin = expectedPin;
        }

        @Override
        public void checkClientTrusted(X509Certificate[] chain, String authType) {
            // Devices authenticate with a bearer token, not a client certificate.
        }

        @Override
        public void checkServerTrusted(X509Certificate[] chain, String authType)
                throws java.security.cert.CertificateException {
            if (chain == null || chain.length == 0) {
                throw new java.security.cert.CertificateException("Server presented no certificate");
            }
            X509Certificate root = chain[chain.length - 1];
            String served = pin(root.getPublicKey());
            if (!served.equals(expectedPin)) {
                throw new java.security.cert.CertificateException(
                        "Server identity changed: expected " + expectedPin + " but saw " + served);
            }
            if (root.getBasicConstraints() < 0) {
                throw new java.security.cert.CertificateException(
                        "The confirmed certificate is not a certificate authority");
            }
            // Absent must mean "no", not "unrestricted": a root that never says
            // it may sign certificates is not a root we will trust to sign one.
            boolean[] usage = root.getKeyUsage();
            if (usage == null) {
                throw new java.security.cert.CertificateException(
                        "The confirmed certificate declares no key usage, so it may not sign certificates");
            }
            if (usage.length <= 5 || !usage[5]) {
                throw new java.security.cert.CertificateException(
                        "The confirmed certificate may not sign other certificates");
            }
            X509Certificate leaf = chain[0];
            leaf.checkValidity();
            // Same rule on the leaf: server-auth must be declared, not assumed.
            java.util.List<String> purposes = leaf.getExtendedKeyUsage();
            if (purposes == null) {
                throw new java.security.cert.CertificateException(
                        "Leaf certificate declares no extended key usage, so it is not a server certificate");
            }
            if (!purposes.contains(SERVER_AUTH)) {
                throw new java.security.cert.CertificateException(
                        "Leaf certificate is not valid for server authentication");
            }
            // Now the actual chain validation, with the confirmed root as anchor.
            try {
                validatorFor(root).checkServerTrusted(chain, authType);
            } catch (java.security.cert.CertificateException error) {
                throw new java.security.cert.CertificateException(
                        "Leaf certificate is not signed by the confirmed root: " + error.getMessage());
            }
        }

        @Override
        public X509Certificate[] getAcceptedIssuers() {
            return new X509Certificate[0];
        }

        private static X509TrustManager validatorFor(X509Certificate anchor) throws java.security.cert.CertificateException {
            try {
                KeyStore store = KeyStore.getInstance(KeyStore.getDefaultType());
                store.load(null, null);
                store.setCertificateEntry("confirmed-anchor", anchor);
                TrustManagerFactory managers =
                        TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
                managers.init(store);
                for (TrustManager manager : managers.getTrustManagers()) {
                    if (manager instanceof X509TrustManager) return (X509TrustManager) manager;
                }
                throw new java.security.cert.CertificateException("No X509 trust manager available");
            } catch (java.security.cert.CertificateException error) {
                throw error;
            } catch (Exception error) {
                throw new java.security.cert.CertificateException("Cannot build a validator: " + error, error);
            }
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
