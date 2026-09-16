package com.biubiu.vpn

import android.net.Network
import org.json.JSONObject
import java.net.URL
import javax.net.ssl.HttpsURLConnection

class ApiError(val code: Int, message: String) : Exception(message) {
    val fatal: Boolean get() = code in setOf(1002, 1004, 1007, 3002)
}

/** One lock serializes refresh and all session changes; normal TLS verification stays enabled. */
class Api(private val vault: CredentialStore, private val exchange: ((String, JSONObject, String?) -> JSONObject)? = null) {
    @Volatile var network: Network? = null
    @Volatile private var connection: HttpsURLConnection? = null
    @Volatile private var cancelled = false
    private var session = vault.read()
    private val commitLock = Any()
    fun cancel() { synchronized(commitLock) { cancelled = true }; connection?.disconnect() }
    private fun save() { synchronized(commitLock) { check(!cancelled) { "操作已取消" }; vault.write(session) } }
    private fun clearSession() { synchronized(commitLock) { check(!cancelled) { "操作已取消" }; session = JSONObject(); vault.clear() } }
    @Synchronized fun saved(): JSONObject = JSONObject(session.toString())
    @Synchronized fun login(server: String, username: String, password: String): Boolean {
        val url = URL(server.trim().trimEnd('/'))
        require(url.protocol == "https" && url.host.isNotEmpty() && url.userInfo == null && url.query == null && url.ref == null && (url.path.isEmpty() || url.path == "/")) { "请输入 HTTPS 服务器地址（不含路径）" }
        val previous = session
        session = JSONObject().put("server", url.toString().trimEnd('/'))
        try {
            val data = post("/auth/login", JSONObject().put("username", username).put("password", password), false)
            session.put("access", data.getString("access_token")).put("refresh", data.getString("refresh_token"))
                .put("mustChange", data.optBoolean("must_change_password"))
            val keys = if (previous.optString("server") == session.getString("server") && previous.optString("username") == username && previous.has("private")) previous else Native.request("keys")
            session.put("private", keys.getString("private")).put("public", keys.getString("public")).put("username", username)
            save()
            return session.getBoolean("mustChange")
        } catch (e: Exception) { session = previous; throw e }
    }
    @Synchronized fun changePassword(old: String, new: String) {
        post("/auth/change-password", JSONObject().put("old_password", old).put("new_password", new))
        clearSession()
    }
    @Synchronized fun logout() {
        try { if (session.has("refresh")) post("/auth/logout", JSONObject().put("refresh_token", session.getString("refresh"))) }
        finally { clearSession() }
    }
    @Synchronized fun register(): JSONObject {
        check(!session.optBoolean("mustChange")) { "请先修改密码" }
        return post("/peers/register", JSONObject().put("wg_public_key", session.getString("public"))
            .put("device_name", "Android ${android.os.Build.MANUFACTURER} ${android.os.Build.MODEL}")
            .put("os_info", "Android ${android.os.Build.VERSION.RELEASE}").put("client_version", "0.1.15")
            .put("capabilities", org.json.JSONArray().put("obfs-v1")))
    }
    @Synchronized fun heartbeat(): JSONObject = post("/peers/heartbeat", JSONObject().put("wg_public_key", session.getString("public")))
    @Synchronized private fun post(path: String, body: JSONObject, auth: Boolean = true): JSONObject {
        try { return once(path, body, auth) }
        catch (e: ApiError) {
            if (auth && e.code == 1002 && session.has("refresh")) {
                try {
                    val data = once("/auth/refresh", JSONObject().put("refresh_token", session.getString("refresh")), false)
                    session.put("access", data.getString("access_token"))
                    if (data.has("refresh_token")) session.put("refresh", data.getString("refresh_token"))
                    save()
                    return once(path, if (path == "/auth/logout") JSONObject().put("refresh_token", session.getString("refresh")) else body, true)
                } catch (refreshError: ApiError) {
                    if (refreshError.fatal) { clearSession() }
                    throw refreshError
                }
            }
            if (auth && e.fatal) { clearSession() }
            throw e
        }
    }
    private fun once(path: String, body: JSONObject, auth: Boolean): JSONObject {
        check(!cancelled) { "操作已取消" }
        if (exchange != null) {
            val response = exchange.invoke(path, body, if (auth) session.getString("access") else null)
            check(!cancelled) { "操作已取消" }
            val code = response.getInt("code")
            if (code != 0) throw ApiError(code, response.optString("message", "请求失败 ($code)"))
            return response.optJSONObject("data") ?: JSONObject()
        }
        val physical = network ?: throw java.io.IOException("没有可用物理网络")
        val conn = physical.openConnection(URL(session.getString("server") + "/api/v1" + path)) as HttpsURLConnection
        connection = conn
        try {
            check(!cancelled) { "操作已取消" }
            conn.instanceFollowRedirects = false
            conn.connectTimeout = 10000; conn.readTimeout = 10000
            conn.requestMethod = "POST"; conn.doOutput = true
            conn.setRequestProperty("Content-Type", "application/json")
            if (auth) conn.setRequestProperty("Authorization", "Bearer " + session.getString("access"))
            conn.outputStream.use { it.write(body.toString().toByteArray(Charsets.UTF_8)) }
            val status = conn.responseCode
            if (status in 300..399) throw java.io.IOException("服务器重定向被拒绝")
            val stream = if (status >= 400) conn.errorStream else conn.inputStream
            val text = stream?.use { stream ->
                val output = java.io.ByteArrayOutputStream(); val buffer = ByteArray(8192)
                while (true) { val count = stream.read(buffer); if (count < 0) break; require(output.size() + count <= 1048576) { "响应过大" }; output.write(buffer, 0, count) }
                output.toString("UTF-8") } ?: ""
            val result = try { JSONObject(text) } catch (_: Exception) {
                if (status == 401) throw ApiError(1002, "登录已过期")
                throw java.io.IOException("服务器响应无效 ($status)")
            }
            check(!cancelled) { "操作已取消" }
            val code = result.getInt("code")
            if (code != 0) throw ApiError(code, result.optString("message", "请求失败 ($code)"))
            if (status !in 200..299) throw java.io.IOException("HTTP $status")
            return result.optJSONObject("data") ?: JSONObject()
        } finally { conn.disconnect(); if (connection === conn) connection = null }
    }
}
