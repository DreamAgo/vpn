package com.biubiu.vpn

import java.util.Locale

/** Shared display units; counters remain raw bytes internally. */
object TrafficFormat {
    fun bytes(value: Long): String = scaled(value.coerceAtLeast(0).toDouble())
    fun rate(bytesPerSecond: Double): String = scaled(bytesPerSecond.takeIf { it.isFinite() && it >= 0 } ?: 0.0) + "/s"
    private fun scaled(value: Double): String = when {
        value >= 1024 * 1024 -> String.format(Locale.ROOT, "%.2f MB", value / (1024 * 1024))
        value >= 1024 -> String.format(Locale.ROOT, "%.1f KB", value / 1024)
        else -> String.format(Locale.ROOT, "%.0f B", value)
    }
    fun details(raw: String): String = raw.lineSequence().joinToString("\n") { line ->
        val match = Regex("^(上传|下载)：([0-9]+) B$").matchEntire(line)
        val count = match?.groupValues?.get(2)?.toLongOrNull()
        if (count != null) "${match.groupValues[1]}：${bytes(count)}" else line
    }
}

/** One meter per tunnel: monotonic elapsed time, never wall-clock or lifetime-average speed. */
class TrafficRateMeter(startedMillis: Long) {
    private var previousMillis = startedMillis
    private var previousTx = 0L
    private var previousRx = 0L
    fun sample(nowMillis: Long, tx: Long, rx: Long): Pair<Double, Double> {
        if (nowMillis <= previousMillis) return 0.0 to 0.0
        val elapsed = (nowMillis - previousMillis).toDouble() / 1000
        val up = if (tx >= previousTx) (tx - previousTx) / elapsed else 0.0
        val down = if (rx >= previousRx) (rx - previousRx) / elapsed else 0.0
        previousMillis = nowMillis; previousTx = tx.coerceAtLeast(0); previousRx = rx.coerceAtLeast(0)
        return up to down
    }
}
