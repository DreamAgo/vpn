package com.biubiu.vpn

import java.net.URI
import java.net.URLDecoder
import org.junit.Assert.*
import org.junit.Test

class FeishuAuthorizationTest {
    private val url = "https://accounts.feishu.cn/open-apis/authen/v1/authorize?app_id=cli_test&redirect_uri=https%3A%2F%2Fvpn.example%2Fcallback&state=a%2Bb%26c&scope=contact%3Auser.base%3Areadonly"

    @Test fun appLinkPreservesEncodedOAuthParameters() {
        val link = URI(FeishuLogin.appLink(url))
        assertEquals("https", link.scheme)
        assertEquals("applink.feishu.cn", link.host)
        assertEquals("/client/web_url/open", link.path)
        assertEquals(url, URLDecoder.decode(link.rawQuery.substringAfter("&url="), "UTF-8"))
    }

    @Test fun installedFeishuNeverLaunchesBrowser() {
        var launches = 0
        assertTrue(FeishuAuthorization.open(url) { target, pkg ->
            launches++
            assertEquals("com.ss.android.lark", pkg)
            assertEquals(FeishuLogin.appLink(url), target)
            true
        })
        assertEquals(1, launches)
    }

    @Test fun unavailableFeishuFallsBackToOriginalAuthorization() {
        val targets = mutableListOf<Pair<String, String?>>()
        assertFalse(FeishuAuthorization.open(url) { target, pkg -> targets.add(target to pkg); pkg == null })
        assertEquals(listOf(FeishuLogin.appLink(url) to "com.ss.android.lark", url to null), targets)
    }

    @Test fun noHandlerGivesActionableError() {
        try { FeishuAuthorization.open(url) { _, _ -> false }; fail() }
        catch (e: LocalFailure) { assertTrue(e.message!!.contains("安装飞书")) }
    }

    @Test fun untrustedAuthorizationNeverLaunches() {
        try { FeishuAuthorization.open("https://evil.example/authorize") { _, _ -> fail(); true }; fail() }
        catch (_: IllegalArgumentException) { }
    }
}
