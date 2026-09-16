package com.biubiu.vpn

class LocalFailure(message: String) : Exception(message)

/** Only fixed events enter this buffer. Untrusted exception text is deliberately discarded. */
object Diagnostics {
    private val entries = ArrayDeque<String>()
    @Synchronized fun event(message: String) {
        val safe = message.replace(Regex("(?i)(password|token|secret|private|authorization)[^\\n]*"), "[敏感信息已隐藏]").take(240)
        entries.addLast("${java.text.SimpleDateFormat("HH:mm:ss", java.util.Locale.ROOT).format(java.util.Date())} $safe")
        while (entries.size > 120) entries.removeFirst()
    }
    @Synchronized fun snapshot(): String = entries.joinToString("\n")
    @Synchronized fun clear() { entries.clear() }
    fun error(error: Exception): String = when (error) {
        is LocalFailure -> error.message ?: "操作失败"
        is ApiError -> when (error.code) { 2002 -> "服务端拒绝连接，请检查权限或在更新页升级客户端"; 1001 -> "账号或密码错误"; 1002, 1004, 1007 -> "登录已失效，请重新登录"; else -> "服务请求失败（${error.code}）" }
        is InterruptedException -> "操作已取消"
        is javax.net.ssl.SSLException -> "服务器证书验证失败"
        is java.io.IOException -> "网络或文件操作失败，请重试"
        else -> "操作未完成，请检查配置或更新包"
    }
}
