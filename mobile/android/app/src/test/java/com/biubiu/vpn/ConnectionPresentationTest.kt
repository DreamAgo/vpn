package com.biubiu.vpn

import org.junit.Assert.*
import org.junit.Test

class ConnectionPresentationTest {
    private val details = "VPN IP：10.1.1.2\n连接时长：3661 秒\n上传：1048576 B\n下载：0 B"

    @Test fun ownershipBeforeHandshakeDoesNotImplyConnected() {
        for (status in listOf("未连接", "已断开", "正在注册设备", "正在进行 WireGuard 握手")) {
            val view = ConnectionPresentation.from(true, status, details)
            assertFalse(view.connected)
            assertTrue(view.pending)
            assertEquals("取消连接", view.actionLabel)
            assertEquals("—", view.uploaded)
        }
    }
    @Test fun liveHandshakeExposesActualTotals() {
        val view = ConnectionPresentation.from(true, "已连接 10.1.1.2", details)
        assertTrue(view.connected)
        assertEquals("01:01:01", view.duration)
        assertEquals("1.00 MB", view.uploaded)
        assertEquals("0 B", view.downloaded)
    }
    @Test fun disconnectedAndRetryDoNotReusePriorTelemetry() {
        for ((running, status) in listOf(false to "已连接 10.1.1.2", false to "已断开", true to "连接中断，1 秒后重试：网络已切换")) {
            val view = ConnectionPresentation.from(running, status, details)
            assertFalse(view.connected)
            assertEquals("—", view.duration)
            assertEquals("—", view.downloaded)
        }
    }
    @Test fun shutdownCannotBePresentedAsReadyToConnect() {
        val view = ConnectionPresentation.from(true, "正在断开", details)
        assertTrue(view.stopping)
        assertFalse(view.pending)
        assertEquals("正在断开…", view.actionLabel)
    }
    @Test fun validationFailurePreservesActualReasonWithoutInferringQuota() {
        val actual = "连接停止：服务请求失败 6001：配置等待重启"
        val view = ConnectionPresentation.from(false, actual, details)
        assertTrue(view.failed)
        assertEquals(actual, view.subtitle)
        assertEquals("重新连接", view.actionLabel)
        assertEquals("—", view.duration)
    }
    @Test fun missingOrMalformedTelemetryIsUnknownRatherThanZero() {
        val view = ConnectionPresentation.from(true, "已连接", "连接时长：x 秒\n上传：-1 B\n下载：999999999999999999999 B")
        assertEquals("—", view.duration)
        assertEquals("—", view.uploaded)
        assertEquals("—", view.downloaded)
    }
}
