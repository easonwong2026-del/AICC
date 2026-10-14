package com.aieink.pokedashboard;

import android.content.SharedPreferences;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import android.util.Base64;
import java.security.KeyStore;
import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;
import java.nio.charset.StandardCharsets;

final class BridgeCredential {
    private static final String ALIAS = "aicc.read-only-bridge";

    private static SecretKey key() throws Exception {
        KeyStore store = KeyStore.getInstance("AndroidKeyStore");
        store.load(null);
        if (store.containsAlias(ALIAS)) return (SecretKey) store.getKey(ALIAS, null);
        KeyGenerator generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
        generator.init(new KeyGenParameterSpec.Builder(ALIAS,
                KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build());
        return generator.generateKey();
    }

    static void save(SharedPreferences preferences, String token, String host) throws Exception {
        Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
        cipher.init(Cipher.ENCRYPT_MODE, key());
        String encrypted = Base64.encodeToString(cipher.doFinal(token.getBytes(StandardCharsets.UTF_8)), Base64.NO_WRAP);
        String nonce = Base64.encodeToString(cipher.getIV(), Base64.NO_WRAP);
        if (!preferences.edit().putString("bridge_secret", encrypted).putString("bridge_nonce", nonce)
                .putString("bridge_host", host).commit()) {
            throw new IllegalStateException("Credential storage failed");
        }
    }

    static String load(SharedPreferences preferences) {
        try {
            if (!java.util.Objects.equals(preferences.getString("bridge_host", null),
                    preferences.getString("host_id", null))) return null;
            String encrypted = preferences.getString("bridge_secret", null);
            String nonce = preferences.getString("bridge_nonce", null);
            if (encrypted == null || nonce == null) return null;
            Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
            cipher.init(Cipher.DECRYPT_MODE, key(), new GCMParameterSpec(128, Base64.decode(nonce, Base64.NO_WRAP)));
            return new String(cipher.doFinal(Base64.decode(encrypted, Base64.NO_WRAP)), StandardCharsets.UTF_8);
        } catch (Exception error) { return null; }
    }
}
