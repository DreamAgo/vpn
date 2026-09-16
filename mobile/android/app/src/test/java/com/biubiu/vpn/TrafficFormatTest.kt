package com.biubiu.vpn

import org.junit.Assert.*
import org.junit.Test

class TrafficFormatTest {
    @Test fun unitsSwitchAtBinaryBoundaries() {
        assertEquals("0 B", TrafficFormat.bytes(0))
        assertEquals("1023 B", TrafficFormat.bytes(1023))
        assertEquals("1.0 KB", TrafficFormat.bytes(1024))
        assertEquals("1.5 KB", TrafficFormat.bytes(1536))
        assertEquals("1.00 MB", TrafficFormat.bytes(1048576))
        assertEquals("1024.00 MB", TrafficFormat.bytes(1073741824))
    }
    @Test fun rateUsesCounterDeltaAndActualInterval() {
        val meter = TrafficRateMeter(1000)
        assertEquals(1024.0 to 524288.0, meter.sample(3000, 2048, 1048576))
        assertEquals(2048.0 to 0.0, meter.sample(4000, 4096, 1048576))
        assertEquals("1.0 KB/s", TrafficFormat.rate(1024.0))
        assertEquals("1.00 MB/s", TrafficFormat.rate(1048576.0))
    }
    @Test fun idleResetAndInvalidIntervalNeverYieldNegativeOrInfiniteRate() {
        val meter = TrafficRateMeter(0)
        meter.sample(1000, 1024, 1024)
        assertEquals(0.0 to 0.0, meter.sample(2000, 1024, 1024))
        assertEquals(0.0 to 0.0, meter.sample(3000, 0, 0))
        assertEquals(0.0 to 0.0, meter.sample(3000, 1024, 1024))
        assertEquals(1024.0 to 1024.0, meter.sample(4000, 1024, 1024))
        assertEquals("0 B/s", TrafficFormat.rate(Double.NaN))
    }
    @Test fun detailsFormatsTotalsWithoutChangingOtherFieldsOrMalformedData() {
        assertEquals("VPN IP：10.1.1.1\n上传：1.0 KB\n下载：2.00 MB\n连接时长：1024 秒",
            TrafficFormat.details("VPN IP：10.1.1.1\n上传：1024 B\n下载：2097152 B\n连接时长：1024 秒"))
        assertEquals("上传：unknown", TrafficFormat.details("上传：unknown"))
    }
}
