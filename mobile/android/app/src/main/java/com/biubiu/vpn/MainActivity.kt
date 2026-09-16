package com.biubiu.vpn

import android.Manifest
import android.app.*
import android.content.Intent
import android.net.ConnectivityManager
import android.net.VpnService
import android.os.*
import android.text.InputType
import android.view.View
import android.widget.*
import java.util.concurrent.Executors

class MainActivity : Activity() {
    private lateinit var api: Api
    private lateinit var server: EditText
    private lateinit var username: EditText
    private lateinit var password: EditText
    private lateinit var state: TextView
    private lateinit var account: TextView
    private val buttons = mutableListOf<Button>()
    private val executor = Executors.newSingleThreadExecutor()
    private val handler = Handler(Looper.getMainLooper())
    private var busy = false
    private var lastTunnelStatus = ""
    private var lastRunning = false
    private val refresh = object : Runnable {
        override fun run() {
            if (!busy && TunnelService.status != lastTunnelStatus) { lastTunnelStatus = TunnelService.status; state.text = lastTunnelStatus }
            val active = TunnelService.running
            if (lastRunning && !active && !busy) reloadAccount()
            lastRunning = active
            server.isEnabled = !busy && !active; username.isEnabled = !busy && !active; password.isEnabled = !busy && !active
            buttons.forEach { it.isEnabled = !busy && (!active || it.text == "断开") }
            handler.postDelayed(this, 1000)
        }
    }
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.setFlags(android.view.WindowManager.LayoutParams.FLAG_SECURE, android.view.WindowManager.LayoutParams.FLAG_SECURE)
        try { api = Api(Vault(this)) } catch (_: Exception) {
            AlertDialog.Builder(this).setTitle("安全凭据不可读取").setMessage("凭据可能损坏或系统密钥已失效。清除本机保存的登录后重新登录。")
                .setPositiveButton("清除并重新登录") { _, _ -> Vault(this).clear(); recreate() }
                .setNegativeButton("关闭") { _, _ -> finish() }.setCancelable(false).show()
            return
        }
        val content = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(36, 48, 36, 36) }
        fun label(text: String, size: Float = 16f) = TextView(this).apply { this.text = text; textSize = size; setPadding(0, 12, 0, 12); content.addView(this) }
        label("易链", 28f)
        label("安全连接企业内网")
        label("版本 ${BuildConfig.VERSION_NAME}", 13f)
        server = EditText(this).apply { hint = "https://vpn.example.com"; inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI; setSingleLine(); content.addView(this) }
        username = EditText(this).apply { hint = "账号"; inputType = InputType.TYPE_CLASS_TEXT; setSingleLine(); content.addView(this) }
        password = EditText(this).apply { hint = "密码"; inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD; setSingleLine(); content.addView(this) }
        val saved = api.saved()
        server.setText(saved.optString("server")); username.setText(saved.optString("username"))
        account = label(if (saved.has("refresh")) "已保存登录：${saved.optString("username")}" else "请先登录")
        state = label(TunnelService.status)
        fun button(text: String, action: () -> Unit) {
            val view = Button(this).apply { this.text = text; setOnClickListener { action() } }
            buttons.add(view); content.addView(view)
        }
        button("登录") {
            val host = server.text.toString(); val user = username.text.toString(); val secret = password.text.toString()
            password.text.clear()
            work {
                val change = api.login(host, user, secret)
                runOnUiThread { account.text = "已登录：$user"; if (change) passwordDialog() }
            }
        }
        button("连接") {
            if (!reloadAccount()) return@button
            if (!api.saved().has("refresh")) { state.text = "请先登录"; return@button }
            if (api.saved().optBoolean("mustChange")) { passwordDialog(); return@button }
            val permission = VpnService.prepare(this)
            if (permission != null) startActivityForResult(permission, 10) else connect()
        }
        button("断开") { startService(Intent(this, TunnelService::class.java).setAction(TunnelService.STOP)) }
        button("修改密码") { passwordDialog() }
        button("退出登录") { work { api.logout(); runOnUiThread { account.text = "已退出登录" } } }
        label("仅授权网段通过 VPN；普通上网使用当前网络。连接状态以 WireGuard 握手为准。", 13f)
        setContentView(ScrollView(this).apply { isFillViewport = true; addView(content) })
        content.setOnApplyWindowInsetsListener { view, insets ->
            view.setPadding(36, 24 + insets.systemWindowInsetTop, 36, 24 + insets.systemWindowInsetBottom); insets
        }
        handler.post(refresh)
    }
    private fun reloadAccount(): Boolean {
        if (busy || TunnelService.running) return false
        try { api = Api(Vault(this)) } catch (_: Exception) {
            state.text = "安全凭据不可读取，请重启应用清除凭据后登录"
            return false
        }
        val saved = api.saved()
        account.text = if (saved.has("refresh")) "已保存登录：${saved.optString("username")}" else "请先登录"
        return true
    }
    private fun work(action: () -> Unit) {
        if (busy || TunnelService.running) return
        if (!reloadAccount()) return
        busy = true; state.text = "处理中…"
        executor.execute {
            try {
                api.network = PhysicalNetwork.choose(getSystemService(ConnectivityManager::class.java))
                action()
                runOnUiThread { state.text = "操作成功" }
            } catch (e: Exception) { runOnUiThread { state.text = e.message ?: "操作失败"; Toast.makeText(this, state.text, Toast.LENGTH_LONG).show() } }
            finally { runOnUiThread { busy = false } }
        }
    }
    private fun passwordDialog() {
        val fields = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(40, 12, 40, 12) }
        fun field(hint: String) = EditText(this).apply { this.hint = hint; inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD; fields.addView(this) }
        val old = field("当前密码"); val new = field("新密码")
        AlertDialog.Builder(this).setTitle("修改密码").setView(fields).setNegativeButton("取消", null).setPositiveButton("保存") { _, _ ->
            val oldSecret = old.text.toString(); val newSecret = new.text.toString()
            work { api.changePassword(oldSecret, newSecret); runOnUiThread { account.text = "密码已修改，请重新登录" } }
        }.show()
    }
    private fun connect() {
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != android.content.pm.PackageManager.PERMISSION_GRANTED) requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 11)
        startForegroundService(Intent(this, TunnelService::class.java))
    }
    @Deprecated("Legacy activity result supports API 26 without AndroidX")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode == 10) { if (resultCode == RESULT_OK) connect() else state.text = "VPN 权限未授予" }
    }
    override fun onResume() { super.onResume(); if (::api.isInitialized && !busy && !TunnelService.running) reloadAccount() }
    override fun onDestroy() { handler.removeCallbacks(refresh); if (::api.isInitialized) api.cancel(); executor.shutdownNow(); super.onDestroy() }
}
