package com.biubiu.vpn

import org.json.JSONObject
import org.json.JSONArray
import org.junit.Assert.*
import org.junit.Test

class ParityTest {
    private fun manifest() = JSONObject().put("version", "0.1.34").put("downloads", JSONArray().put(JSONObject()
        .put("name", "vpn-android-universal-0.1.34.apk")
        .put("url", "https://vpn.example/updates/releases/00000000-0000-0000-0000-000000000000/vpn-android-universal-0.1.34.apk")
        .put("size", 123).put("digest", "sha256:" + "ab".repeat(32))))
    private fun rejected(block: () -> Unit) { try { block(); fail("should reject") } catch (_: IllegalArgumentException) {} catch (_: IllegalStateException) {} catch (_: LocalFailure) {} }
    @Test fun updateRequiresNewCanonicalSameOriginPackage() {
        assertEquals(1034L, ClientUpdates.versionCode("0.1.34"))
        assertNotNull(ClientUpdates.parse("https://vpn.example", manifest(), "0.1.33"))
        assertNull(ClientUpdates.parse("https://vpn.example", manifest(), "0.1.34"))
        for (url in listOf("http://vpn.example/a.apk", "https://evil.example/a.apk", "https://vpn.example:444/a.apk", "https://vpn.example/updates/releases/../a.apk", "https://vpn.example@evil.example/a.apk")) {
            val data = manifest(); data.getJSONArray("downloads").getJSONObject(0).put("url", url)
            rejected { ClientUpdates.parse("https://vpn.example", data, "0.1.33") }
        }
    }
    @Test fun rejectsMissingDuplicateOversizeAndInvalidDigest() {
        val missing = manifest().put("downloads", JSONArray()); rejected { ClientUpdates.parse("https://vpn.example", missing, "0.1.33") }
        val duplicate = manifest(); duplicate.getJSONArray("downloads").put(duplicate.getJSONArray("downloads").getJSONObject(0)); rejected { ClientUpdates.parse("https://vpn.example", duplicate, "0.1.33") }
        for ((key, value) in listOf("size" to 268435457, "digest" to "sha256:bad", "name" to "wrong.apk")) {
            val data = manifest(); data.getJSONArray("downloads").getJSONObject(0).put(key, value); rejected { ClientUpdates.parse("https://vpn.example", data, "0.1.33") }
        }
    }
    @Test fun versionBoundsAndFeishuHostAreStrict() {
        listOf("1.0.1000", "2101.0.0", "0.1.34-beta", "v0.1.34", "0.01.34").forEach { rejected { ClientUpdates.versionCode(it) } }
        FeishuLogin.validateUrl("https://accounts.feishu.cn/path?state=x")
        listOf("http://accounts.feishu.cn/", "https://accounts.feishu.cn.evil/", "https://user@accounts.feishu.cn/", "https://accounts.feishu.cn:8443/").forEach { rejected { FeishuLogin.validateUrl(it) } }
    }
    @Test fun cancelledAndReplacedOperationsCannotDeliverLateCallbacks() {
        val gate = OperationGate(); val first = gate.begin(); assertTrue(gate.accepts(first)); gate.cancel(); assertFalse(gate.accepts(first))
        val next = gate.begin(); assertTrue(gate.accepts(next)); assertFalse(gate.accepts(first)); gate.begin(); assertFalse(gate.accepts(next))
    }
    @Test fun passwordRequiresLengthLettersAndDigits() {
        assertTrue(MainActivity.validPassword("abcdef12")); assertFalse(MainActivity.validPassword("abcdefg")); assertFalse(MainActivity.validPassword("12345678")); assertFalse(MainActivity.validPassword("abcdefgh"))
    }
    @Test fun logsBoundAndRedactSecretsAndErrors() {
        Diagnostics.clear(); Diagnostics.event("password=supersecret token=secret"); assertFalse(Diagnostics.snapshot().contains("supersecret"))
        repeat(200) { Diagnostics.event("event $it") }; assertEquals(120, Diagnostics.snapshot().lines().size)
        assertFalse(Diagnostics.error(Exception("secret-value")).contains("secret-value"))
    }
    @Test fun minimumVersionRejectionKeepsCredentials() {
        val initial = JSONObject().put("server", "https://vpn.example").put("access", "a").put("refresh", "r").put("public", "p")
        var cleared = false
        val api = Api(object : CredentialStore { override fun read() = initial; override fun write(value: JSONObject) {}; override fun clear() { cleared = true } }) { _, _, _ -> JSONObject().put("code", 2002) }
        try { api.heartbeat(); fail() } catch (e: ApiError) { assertFalse(e.fatal) }
        assertFalse(cleared); assertEquals("r", api.saved().getString("refresh"))
    }
}
