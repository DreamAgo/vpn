package com.biubiu.vpn

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class ValidationErrorTest {
    @Test fun serverValidationReasonReachesUserWithoutErasingSession() {
        val reason = "参数校验失败: 已达终端数量上限（1）且均在线；请先下线一台终端或联系管理员提升上限"
        var cleared = false
        val api = Api(object : CredentialStore {
            override fun read() = JSONObject().put("server", "https://vpn.example").put("access", "a").put("refresh", "r").put("public", "p")
            override fun write(value: JSONObject) { fail("validation must not refresh") }
            override fun clear() { cleared = true }
        }) { _, _, _ -> JSONObject().put("code", 6001).put("message", reason) }
        try { api.heartbeat(); fail("expected rejection") } catch (e: ApiError) {
            assertTrue(Diagnostics.error(e).contains("已达终端数量上限（1）"))
            assertEquals("服务端错误（6001）", Diagnostics.logError(e))
            assertTrue(e.stopsTunnel)
            assertFalse(e.fatal)
        }
        assertFalse(cleared)
        assertEquals("r", api.saved().getString("refresh"))
        assertFalse(ApiError(4001, "请求过快").stopsTunnel)
    }
    @Test fun validationReasonsRemainBoundedAndSensitiveValuesAreHidden() {
        for (message in listOf("password=abc123", "access_token=abc123", "https://user:abc123@example.com", "abcdefgh.abcdefgh.abcdefgh", "密码：abc123", "private_key=" + "a".repeat(43) + "=")) {
            assertEquals("服务端说明包含敏感信息，已隐藏", Diagnostics.serverReason(message))
        }
        assertEquals("飞书账号未提供有效邮箱", Diagnostics.serverReason("飞书账号未提供有效邮箱"))
        assertEquals("服务端未提供具体原因", Diagnostics.serverReason(" "))
        assertEquals(512, Diagnostics.serverReason("错".repeat(2000)).length)
        assertEquals("参数 不正确", Diagnostics.serverReason("参数\n\u202e不正确"))
    }
}
