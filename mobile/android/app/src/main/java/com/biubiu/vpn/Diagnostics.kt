package com.biubiu.vpn

class LocalFailure(message: String) : Exception(message)

/** Only fixed events enter this buffer. Untrusted exception text is deliberately discarded. */
object Diagnostics {
    /** Service messages are useful to the user, but must not echo credentials. */
    fun serverReason(message: String?): String {
        val value = message.orEmpty().trim()
        if (value.isEmpty()) return "服务端未提供具体原因"
        val lower = value.lowercase(java.util.Locale.ROOT)
        val markers = listOf("password", "token", "authorization", "bearer", "private", "secret", "credential", "api_key", "cookie", "密码=", "密码：", "密钥=", "密钥：", "令牌=", "令牌：")
        if (markers.any { lower.contains(it) } ||
            Regex("https?://[^\\s/]*@", RegexOption.IGNORE_CASE).containsMatchIn(value) ||
            Regex("[A-Za-z0-9_-]{8,}\\.[A-Za-z0-9_-]{8,}\\.[A-Za-z0-9_-]{8,}").containsMatchIn(value) ||
            Regex("[A-Za-z0-9+/]{43}=").containsMatchIn(value)) return "服务端说明包含敏感信息，已隐藏"
        return value.replace(Regex("[\\p{Cc}\\p{Cf}]+"), " ").take(512)
    }
    private val entries = ArrayDeque<String>()
    @Synchronized fun event(message: String) {
        val safe = message.replace(Regex("(?i)(password|token|secret|private|authorization)[^\\n]*"), "[敏感信息已隐藏]").take(240)
        entries.addLast("${java.text.SimpleDateFormat("HH:mm:ss", java.util.Locale.ROOT).format(java.util.Date())} $safe")
        while (entries.size > 120) entries.removeFirst()
    }
    @Synchronized fun snapshot(): String = entries.joinToString("\n")
    @Synchronized fun clear() { entries.clear() }
    fun logError(error: Exception): String = if (error is ApiError) "服务端错误（${error.code}）" else this.error(error)
    fun error(error: Exception): String = when (error) {
        is LocalFailure -> error.message ?: "操作失败"
        is ApiError -> when (error.code) {
            6001 -> "参数校验失败（6001）：${serverReason(error.message)}"
            2002 -> "服务端拒绝连接，请检查权限或在「我的 → 应用更新」升级客户端"
            1001 -> "账号或密码错误"
            1002, 1004, 1007 -> "登录已失效，请重新登录"
            else -> "服务请求失败（${error.code}）"
        }
        is InterruptedException -> "操作已取消"
        is javax.net.ssl.SSLException -> "服务器证书验证失败"
        is java.io.IOException -> "网络或文件操作失败，请重试"
        else -> "操作未完成，请检查配置或更新包"
    }
}
