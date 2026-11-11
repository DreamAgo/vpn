package com.biubiu.vpn

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import org.json.JSONObject
import java.security.KeyStore
import java.security.MessageDigest
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Only ciphertext reaches preferences; passwords are never persisted. */
interface CredentialStore {
    fun read(): JSONObject
    fun write(value: JSONObject)
    fun clear()
    fun deviceIdentity(server: String, username: String, create: () -> JSONObject): JSONObject = create()
}

class Vault(context: Context) : CredentialStore {
    private val prefs = context.getSharedPreferences("vault", Context.MODE_PRIVATE)
    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey("vpn-v1", null) as? SecretKey)?.let { return it }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder("vpn-v1", KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build())
        }.generateKey()
    }
    private fun readEncrypted(name: String): JSONObject? {
        val raw = prefs.getString(name, null) ?: return null
        val bytes = Base64.decode(raw, Base64.NO_WRAP)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, bytes.copyOfRange(0, 12)))
        return JSONObject(String(cipher.doFinal(bytes.copyOfRange(12, bytes.size)), Charsets.UTF_8))
    }
    private fun writeEncrypted(name: String, value: JSONObject) {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val bytes = cipher.iv + cipher.doFinal(value.toString().toByteArray(Charsets.UTF_8))
        check(prefs.edit().putString(name, Base64.encodeToString(bytes, Base64.NO_WRAP)).commit()) { "无法保存安全凭据" }
    }
    override fun read(): JSONObject = synchronized(lock) { readEncrypted("session") ?: JSONObject() }
    override fun write(value: JSONObject) = synchronized(lock) { writeEncrypted("session", value) }
    override fun clear() = synchronized(lock) { check(prefs.edit().remove("session").commit()) }

    override fun deviceIdentity(server: String, username: String, create: () -> JSONObject): JSONObject = synchronized(lock) {
        val digest = MessageDigest.getInstance("SHA-256").digest((server + "\u0000" + username).toByteArray(Charsets.UTF_8))
        val name = "device-" + digest.toHex()
        readEncrypted(name) ?: create().also { writeEncrypted(name, it) }
    }

    companion object { private val lock = Any() }
}


internal fun ByteArray.toHex(): String = joinToString("") { (it.toInt() and 255).toString(16).padStart(2, '0') }
