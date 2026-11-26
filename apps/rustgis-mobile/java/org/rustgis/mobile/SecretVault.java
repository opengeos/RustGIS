package org.rustgis.mobile;

/**
 * Secrets kept for the Rust side's secret store (src/secrets.rs): each one encrypted with
 * AES-GCM under a key in the Android Keystore, made once and never leaving it, and kept in the
 * app's private preferences as "1.<nonce>.<ciphertext>" (Base64), the account name bound to it.
 * Nothing is kept unencrypted: when the keystore can't be used, the Rust side says so and the
 * secret lasts for the session. An entry a newer RustGIS wrote (a version above 1) is neither
 * read, written over nor removed.
 *
 * Each call answers with a line: "ok" (with the secret after a tab, for a load that found one),
 * "none" (a load that found none), "unavailable" or "failed", each with the reason after a tab.
 * Called on any thread: one call at a time, so a write and the putting back of a write that
 * failed see no other call's entry in between.
 */
final class SecretVault {
    /** The keystore's name for the one key. */
    private static final String KEY = "rustgis-secrets";
    /** The app's private preferences the encrypted secrets are kept in. */
    private static final String FILE = "rustgis-secrets";
    /** The version of the entries this one writes. */
    private static final int VERSION = 1;
    private static final String CIPHER = "AES/GCM/NoPadding";
    private static final int TAG_BITS = 128;
    private static final String NEWER = "failed\tit was kept by a newer RustGIS: update RustGIS to use it";
    private static final String UNWRITTEN = "failed\tthe app's storage can't be written to";
    private static final String UNCERTAIN =
            "uncertain\tthe app's storage can't be written to, and the secret as it was couldn't be put back: it may still be kept";

    private SecretVault() {}

    static synchronized String load(android.content.Context context, String account) {
        try {
            String kept = preferences(context).getString(account, null);
            if (kept == null) {
                return "none";
            }
            int version = version(kept);
            if (version > VERSION) {
                return NEWER;
            }
            String[] parts = kept.split("\\.", -1);
            if (version != VERSION || parts.length != 3) {
                return "failed\tthe kept secret is damaged";
            }
            javax.crypto.SecretKey key = key(false);
            if (key == null) {
                // Restored to another device, say: the key never leaves the one it was made on.
                return "failed\tthe kept secret can't be decrypted: the keystore no longer has its key";
            }
            byte[] iv = android.util.Base64.decode(parts[1], android.util.Base64.NO_WRAP);
            byte[] sealed = android.util.Base64.decode(parts[2], android.util.Base64.NO_WRAP);
            javax.crypto.Cipher cipher = javax.crypto.Cipher.getInstance(CIPHER);
            cipher.init(javax.crypto.Cipher.DECRYPT_MODE, key, new javax.crypto.spec.GCMParameterSpec(TAG_BITS, iv));
            cipher.updateAAD(account.getBytes(java.nio.charset.StandardCharsets.UTF_8));
            byte[] plain = cipher.doFinal(sealed);
            return "ok\t" + new String(plain, java.nio.charset.StandardCharsets.UTF_8);
        } catch (java.security.KeyStoreException | java.security.NoSuchProviderException | java.io.IOException
                | java.security.ProviderException e) {
            return "unavailable\t" + reason(e);
        } catch (java.security.GeneralSecurityException | IllegalArgumentException | ClassCastException e) {
            // A damaged entry, or one sealed with a key the keystore no longer has.
            return "failed\tthe kept secret can't be decrypted: " + reason(e);
        } catch (RuntimeException e) {
            return "failed\t" + reason(e);
        }
    }

    static synchronized String save(android.content.Context context, String account, String value) {
        try {
            android.content.SharedPreferences preferences = preferences(context);
            String before = kept(preferences, account);
            if (version(before) > VERSION) {
                return NEWER;
            }
            javax.crypto.Cipher cipher = javax.crypto.Cipher.getInstance(CIPHER);
            // The keystore chooses the nonce, a new one for each secret.
            cipher.init(javax.crypto.Cipher.ENCRYPT_MODE, key(true));
            // Bound to its account: an entry copied under another account doesn't decrypt.
            cipher.updateAAD(account.getBytes(java.nio.charset.StandardCharsets.UTF_8));
            byte[] sealed = cipher.doFinal(value.getBytes(java.nio.charset.StandardCharsets.UTF_8));
            String kept = VERSION + "." + android.util.Base64.encodeToString(cipher.getIV(), android.util.Base64.NO_WRAP) + "."
                    + android.util.Base64.encodeToString(sealed, android.util.Base64.NO_WRAP);
            return write(preferences, account, kept, before);
        } catch (java.security.KeyStoreException | java.security.NoSuchProviderException | java.io.IOException
                | java.security.ProviderException e) {
            return "unavailable\t" + reason(e);
        } catch (ClassCastException e) {
            return "failed\tthe kept secret is damaged";
        } catch (java.security.GeneralSecurityException | RuntimeException e) {
            return "failed\t" + reason(e);
        }
    }

    static synchronized String delete(android.content.Context context, String account) {
        try {
            android.content.SharedPreferences preferences = preferences(context);
            String before = kept(preferences, account);
            if (version(before) > VERSION) {
                return NEWER;
            }
            return before == null ? "ok" : write(preferences, account, null, before);
        } catch (ClassCastException e) {
            return "failed\tthe kept secret is damaged";
        } catch (RuntimeException e) {
            return "failed\t" + reason(e);
        }
    }

    /**
     * Writes the account's entry (null removes it) and answers how it went. A write that isn't
     * confirmed puts the entry as it was back (Android keeps what it was given in memory, and
     * would write it with a later save of another account), so a secret said not to be kept isn't;
     * when that fails too, the answer says it may be.
     */
    private static String write(android.content.SharedPreferences preferences, String account, String kept, String before) {
        if (put(preferences.edit(), account, kept).commit()) {
            return "ok";
        }
        return put(preferences.edit(), account, before).commit() ? UNWRITTEN : UNCERTAIN;
    }

    private static android.content.SharedPreferences.Editor put(android.content.SharedPreferences.Editor editor, String account, String kept) {
        return kept == null ? editor.remove(account) : editor.putString(account, kept);
    }

    /** The account's entry, or null; one that isn't text (not this vault's) throws ClassCastException. */
    private static String kept(android.content.SharedPreferences preferences, String account) {
        return preferences.getString(account, null);
    }

    /** The version an entry was written in: the number before its first point, 0 if none. */
    private static int version(String kept) {
        if (kept == null) {
            return 0;
        }
        int dot = kept.indexOf('.');
        try {
            return dot <= 0 ? 0 : Integer.parseInt(kept.substring(0, dot));
        } catch (NumberFormatException e) {
            return 0;
        }
    }

    private static android.content.SharedPreferences preferences(android.content.Context context) {
        return context.getApplicationContext().getSharedPreferences(FILE, android.content.Context.MODE_PRIVATE);
    }

    /** The keystore's key, made the first time it is asked for when {@code make}; else null when there is none. */
    private static synchronized javax.crypto.SecretKey key(boolean make) throws java.security.GeneralSecurityException, java.io.IOException {
        java.security.KeyStore store = java.security.KeyStore.getInstance("AndroidKeyStore");
        store.load(null);
        java.security.Key key = store.getKey(KEY, null);
        if (key instanceof javax.crypto.SecretKey) {
            return (javax.crypto.SecretKey) key;
        }
        if (!make) {
            return null;
        }
        javax.crypto.KeyGenerator maker = javax.crypto.KeyGenerator.getInstance(
                android.security.keystore.KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
        maker.init(new android.security.keystore.KeyGenParameterSpec.Builder(KEY,
                android.security.keystore.KeyProperties.PURPOSE_ENCRYPT | android.security.keystore.KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(android.security.keystore.KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(android.security.keystore.KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build());
        return maker.generateKey();
    }

    /** What went wrong, on one line. */
    private static String reason(Throwable e) {
        String message = e.getMessage() == null ? e.getClass().getSimpleName() : e.getMessage();
        return message.replaceAll("[\\t\\r\\n]", " ");
    }
}
