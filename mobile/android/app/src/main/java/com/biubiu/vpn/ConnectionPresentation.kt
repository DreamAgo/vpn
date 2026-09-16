package com.biubiu.vpn

import java.util.Locale

/** Display-only snapshot. Ownership alone never means that a handshake succeeded. */
data class ConnectionPresentation(
    val connected: Boolean,
    val pending: Boolean,
    val stopping: Boolean,
    val failed: Boolean,
    val title: String,
    val subtitle: String,
    val actionLabel: String,
    val duration: String,
    val uploaded: String,
    val downloaded: String,
) {
    companion object {
        fun from(running: Boolean, status: String, details: String): ConnectionPresentation {
            val connected = running && status.startsWith("已连接")
            val stopping = running && status == "正在断开"
            val failed = status.startsWith("连接停止") || status.startsWith("启动失败")
            val pending = running && !connected && !stopping && !failed
            val fields = if (connected) details.lineSequence().mapNotNull { line ->
                val separator = line.indexOf('：')
                if (separator < 0) null else line.substring(0, separator) to line.substring(separator + 1).trim()
            }.toMap() else emptyMap()
            fun number(key: String): Long? = fields[key]?.substringBefore(' ')?.toLongOrNull()?.takeIf { it >= 0 }
            return ConnectionPresentation(
                connected, pending, stopping, failed,
                when {
                    connected -> "工作网络，已连接"
                    stopping -> "正在断开…"
                    failed -> "连接需要你的帮助"
                    pending && status.startsWith("连接中断") -> "正在恢复连接"
                    pending -> "正在连接…"
                    else -> "随时，安心连接"
                },
                when {
                    connected -> "企业资源已就绪，开始专注工作吧"
                    stopping -> "正在安全关闭连接，请稍候"
                    failed || pending -> status.takeUnless { it == "未连接" || it == "已断开" } ?: "正在准备安全连接"
                    else -> "安全访问企业内网，普通上网不受影响"
                },
                when {
                    stopping -> "正在断开…"
                    connected -> "断开连接"
                    pending -> "取消连接"
                    failed -> "重新连接"
                    else -> "连接工作网络"
                },
                number("连接时长")?.let { elapsed ->
                    if (elapsed >= 3600) String.format(Locale.ROOT, "%02d:%02d:%02d", elapsed / 3600, elapsed / 60 % 60, elapsed % 60)
                    else String.format(Locale.ROOT, "%02d:%02d", elapsed / 60, elapsed % 60)
                } ?: "—",
                number("上传")?.let(TrafficFormat::bytes) ?: "—",
                number("下载")?.let(TrafficFormat::bytes) ?: "—",
            )
        }
    }
}
