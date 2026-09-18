package com.biubiu.vpn

/** Explicit package keeps Android's default browser from intercepting the Feishu AppLink. */
object FeishuAuthorization {
    fun open(url: String, launch: (url: String, packageName: String?) -> Boolean): Boolean {
        val appLink = FeishuLogin.appLink(url)
        if (launch(appLink, "com.ss.android.lark")) return true
        if (!launch(url, null)) throw LocalFailure("无法打开飞书或浏览器，请安装飞书后重试")
        return false
    }
}
