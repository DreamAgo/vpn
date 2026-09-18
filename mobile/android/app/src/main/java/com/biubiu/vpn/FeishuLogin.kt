package com.biubiu.vpn

import org.json.JSONObject
import java.net.URI
import java.net.URLEncoder

/** Owned by one Activity operation; destruction and explicit cancel revoke its right to commit. */
class FeishuLogin(private val api: Api) {
    fun run(server: String, openAuthorization: (String) -> Unit) {
        val origin = Api.validServer(server)
        if (!api.publicRequest(origin, "/auth/feishu/config", method = "GET").optBoolean("enabled")) throw LocalFailure("服务器未启用飞书登录，请使用密码登录")
        val start = api.publicRequest(origin, "/auth/feishu/start?client=android")
        val url = start.getString("authorization_url")
        validateUrl(url)
        val token = start.getString("poll_token")
        val seconds = start.getLong("expires_in").coerceIn(1, 300)
        val deadline = System.nanoTime() + seconds * 1_000_000_000
        openAuthorization(url)
        while (System.nanoTime() < deadline) {
            Thread.sleep(1500)
            if (System.nanoTime() >= deadline) break
            val result = api.publicRequest(origin, "/auth/feishu/poll", JSONObject().put("poll_token", token))
            when (result.getString("status")) {
                "complete" -> {
                    if (System.nanoTime() >= deadline) { api.revokeFeishu(origin, result); throw LocalFailure("飞书登录已超时，请重试") }
                    api.acceptFeishu(origin, result)
                    return
                }
                "pending" -> Unit
                else -> error("无效飞书登录状态")
            }
        }
        throw LocalFailure("飞书登录已超时，请重试")
    }
    companion object {
        /** Official OAuth + AppLink flow; the server still owns state and the HTTPS callback. */
        fun appLink(value: String): String {
            validateUrl(value)
            return "https://applink.feishu.cn/client/web_url/open?mode=window&url=" +
                URLEncoder.encode(value, "UTF-8").replace("+", "%20")
        }

        fun validateUrl(value: String) {
            val uri = URI(value)
            require(uri.scheme == "https" && uri.host == "accounts.feishu.cn" && uri.rawUserInfo == null && (uri.port == -1 || uri.port == 443)) { "服务端返回的飞书授权地址不可信" }
        }
    }
}
