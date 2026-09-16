package com.biubiu.vpn

import android.Manifest
import android.app.*
import android.content.*
import android.net.*
import android.os.*
import android.provider.Settings
import android.text.InputType
import android.widget.*
import java.io.File
import java.util.concurrent.Executors

class MainActivity : Activity() {
    private lateinit var api: Api
    private lateinit var server: EditText
    private lateinit var username: EditText
    private lateinit var password: EditText
    private lateinit var state: TextView
    private lateinit var account: TextView
    private lateinit var detail: TextView
    private lateinit var updateText: TextView
    private lateinit var logText: TextView
    private val mutableButtons = mutableListOf<Button>()
    private val executor = Executors.newSingleThreadExecutor()
    private val handler = Handler(Looper.getMainLooper())
    private var busy = false
    private var destroyed = false
    private val gate = OperationGate()
    private var visiblePage = 0
    @Volatile private var operationApi: Api? = null
    private val taskEpoch = ThreadLocal<Int>()
    private var lastStatus = ""
    private var connectedServer = ""
    private var vpnPermissionTicket: Int? = null
    private var operation: java.util.concurrent.Future<*>? = null
    private var updater: ClientUpdates? = null
    private var candidate: UpdatePackage? = null
    private var ready: File? = null
    private var pendingInstall = false
    private var actionAfterStop: (() -> Unit)? = null
    private val refresh = object : Runnable {
        override fun run() {
            val active = TunnelService.running
            if (!busy && TunnelService.status != lastStatus) { lastStatus = TunnelService.status; state.text = lastStatus }
            detail.text = "服务器：$connectedServer\n" + if (active) TunnelService.details else "${TunnelService.status}\n${TunnelService.details}"
            server.isEnabled = !busy && !active; username.isEnabled = !busy && !active; password.isEnabled = !busy && !active
            mutableButtons.forEach { it.isEnabled = !busy && !active }
            if (!active && actionAfterStop != null) { val next = actionAfterStop; actionAfterStop = null; next?.invoke() }
            if (!busy && !active) reloadAccount()
            if (visiblePage == 3) { val logs = Diagnostics.snapshot(); if (logText.text.toString() != logs) logText.text = logs }
            handler.postDelayed(this, 1000)
        }
    }
    private fun ui(action: () -> Unit) { val ticket = taskEpoch.get() ?: gate.current(); runOnUiThread { if (!destroyed && gate.accepts(ticket)) action() } }
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.setFlags(android.view.WindowManager.LayoutParams.FLAG_SECURE, android.view.WindowManager.LayoutParams.FLAG_SECURE)
        try { api = Api(Vault(this)) } catch (_: Exception) {
            AlertDialog.Builder(this).setTitle("安全凭据不可读取").setMessage("清除本机保存的登录后重新登录。")
                .setPositiveButton("清除并重新登录") { _, _ -> Vault(this).clear(); recreate() }.setNegativeButton("关闭") { _, _ -> finish() }.setCancelable(false).show(); return
        }
        val root = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(24, 32, 24, 24) }
        fun label(parent: LinearLayout, text: String, size: Float = 16f) = TextView(this).apply { this.text = text; textSize = size; setPadding(0, 10, 0, 10); parent.addView(this) }
        label(root, "易链 · ${BuildConfig.VERSION_NAME}", 24f)
        state = label(root, TunnelService.status)
        val nav = LinearLayout(this); root.addView(nav)
        val pages = android.widget.ViewFlipper(this); root.addView(pages, LinearLayout.LayoutParams(-1, 0, 1f))
        fun page(title: String): LinearLayout {
            val index = pages.childCount
            nav.addView(Button(this).apply { text = title; textSize = 12f; setOnClickListener { pages.displayedChild = index; visiblePage = index; if (::logText.isInitialized) logText.text = Diagnostics.snapshot() } }, LinearLayout.LayoutParams(0, -2, 1f))
            val content = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
            pages.addView(ScrollView(this).apply { addView(content) }); return content
        }
        fun button(parent: LinearLayout, text: String, mutable: Boolean = false, action: () -> Unit): Button = Button(this).apply {
            this.text = text; setOnClickListener { action() }; parent.addView(this); if (mutable) mutableButtons.add(this)
        }
        val connectPage = page("连接")
        detail = label(connectPage, TunnelService.details)
        button(connectPage, "连接", true) {
            if (busy || actionAfterStop != null || TunnelService.running) return@button
            if (!reloadAccount() || !api.saved().has("refresh")) { state.text = "请先在账号页登录"; return@button }
            if (api.saved().optBoolean("mustChange")) { passwordDialog(); return@button }
            val permission = VpnService.prepare(this)
            if (permission != null) { vpnPermissionTicket = gate.current(); startActivityForResult(permission, 10) } else connect()
        }
        button(connectPage, "断开") { stopTunnel() }
        button(connectPage, "复制连接详情") {
            getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("易链连接", "$connectedServer\n${TunnelService.details}"))
            Toast.makeText(this, "连接详情已复制", Toast.LENGTH_SHORT).show()
        }
        button(connectPage, "系统 VPN 设置") { startActivity(Intent(Settings.ACTION_VPN_SETTINGS)) }
        label(connectPage, "仅授权网段通过 VPN，普通上网使用当前网络。连接以握手为准。Android 通过前台通知保持服务；暂不支持开机自动连接或始终开启 VPN。", 13f)
        val updatePage = page("更新")
        updateText = label(updatePage, "使用账号页配置的 HTTPS 服务器检查更新。")
        button(updatePage, "检查更新") {
            checkUpdates(server.text.toString())
        }
        button(updatePage, "下载更新") {
            val update = candidate ?: return@button
            if (busy) return@button
            AlertDialog.Builder(this).setTitle("下载 ${update.version}").setMessage("下载 ${update.size / 1024} KiB，完成校验后可交给系统安装。")
                .setNegativeButton("取消", null).setPositiveButton("下载") { _, _ ->
                    val host = server.text.toString()
                    work("下载与校验", true) {
                        require(Api.validServer(host) == update.server)
                        val client = ClientUpdates(this, PhysicalNetwork.choose(getSystemService(ConnectivityManager::class.java)) ?: error("没有网络")); updater = client
                        var lastPercent = -1
                        val file = client.download(update) { done, total -> val percent = (done * 100 / total).toInt(); if (percent != lastPercent) { lastPercent = percent; ui { updateText.text = "下载中 $percent%" } } }
                        ui { ready = file; updateText.text = "${update.version} 校验通过，点击安装更新" }
                    }
                }.show()
        }
        button(updatePage, "安装更新") { if (!busy && ready != null) afterDisconnect { install() } }
        val accountPage = page("账号")
        fun field(hint: String, type: Int) = EditText(this).apply { this.hint = hint; inputType = type; setSingleLine(); accountPage.addView(this) }
        server = field("https://vpn.example.com", InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI)
        username = field("账号", InputType.TYPE_CLASS_TEXT)
        password = field("密码", InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD).apply { isSaveEnabled = false }
        val saved = api.saved(); connectedServer = saved.optString("server"); server.setText(saved.optString("server")); username.setText(saved.optString("username"))
        account = label(accountPage, if (saved.has("refresh")) "已登录：${saved.optString("username")}" else "请先登录")
        button(accountPage, "密码登录", true) {
            val host = server.text.toString(); val user = username.text.toString(); val secret = password.text.toString(); password.text.clear()
            work("密码登录") { client -> val change = client.login(host, user, secret); ui { reloadAccount(); if (change) passwordDialog() } }
        }
        button(accountPage, "飞书登录", true) {
            val host = server.text.toString()
            work("飞书登录") { client -> FeishuLogin(client).run(host) { url ->
                val latch = java.util.concurrent.CountDownLatch(1); var failure: Exception? = null
                ui { try { startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE)); state.text = "在浏览器授权后返回，可点击取消操作" } catch (e: Exception) { failure = e } finally { latch.countDown() } }
                check(latch.await(5, java.util.concurrent.TimeUnit.SECONDS)); failure?.let { throw it }
            }; ui { reloadAccount() } }
        }
        button(accountPage, "修改密码") { if (!busy) afterDisconnect { passwordDialog() } }
        button(accountPage, "退出登录") { if (!busy) afterDisconnect { work("退出登录") { client -> client.logout(); ui { reloadAccount() } } } }
        label(accountPage, "连接中账号只读；修改密码或退出登录前会断开 VPN。", 13f)
        val diagnosticsPage = page("诊断")
        logText = label(diagnosticsPage, Diagnostics.snapshot(), 13f)
        button(diagnosticsPage, "刷新日志") { logText.text = Diagnostics.snapshot() }
        button(diagnosticsPage, "复制诊断") {
            val text = "易链 ${BuildConfig.VERSION_NAME}\nAndroid ${Build.VERSION.RELEASE}\n${TunnelService.status}\n${TunnelService.details}\n${Diagnostics.snapshot()}"
            getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("易链诊断", text))
            Toast.makeText(this, "诊断已复制（包含内网地址）", Toast.LENGTH_SHORT).show()
        }
        button(diagnosticsPage, "清空日志") { Diagnostics.clear(); logText.text = "" }
        button(root, "取消操作") { cancelOperation() }
        setContentView(root)
        root.setOnApplyWindowInsetsListener { view, insets -> view.setPadding(24, 16 + insets.systemWindowInsetTop, 24, 16 + insets.systemWindowInsetBottom); insets }
        handler.post(refresh)
        if (saved.has("refresh") && !saved.optBoolean("mustChange")) handler.post { checkUpdates(saved.getString("server")) }
    }
    private fun checkUpdates(host: String) {
        work("更新检查", allowConnected = true) {
            val client = ClientUpdates(this, PhysicalNetwork.choose(getSystemService(ConnectivityManager::class.java)) ?: throw LocalFailure("没有可用网络")); updater = client
            val update = client.check(host)
            ui { candidate = update; ready = null; updateText.text = if (update == null) "当前已是最新版本" else "可用版本 ${update.version}\n${update.size / 1024} KiB\n${update.notes}" }
        }
    }
    private fun reloadAccount(): Boolean {
        if (TunnelService.running) return false
        try { api = Api(Vault(this)) } catch (_: Exception) { state.text = "安全凭据不可读取"; return false }
        val saved = api.saved(); connectedServer = saved.optString("server"); account.text = if (saved.has("refresh")) "已登录：${saved.optString("username")}" else "请先登录"
        return true
    }
    private fun work(stage: String, allowConnected: Boolean = false, action: (Api) -> Unit) {
        if (busy || (!allowConnected && TunnelService.running)) return
        if (!allowConnected && !reloadAccount()) return
        busy = true; val ticket = gate.begin(); state.text = "$stage…"; Diagnostics.event("$stage 开始")
        val client = api
        operationApi = client
        operation = executor.submit {
            taskEpoch.set(ticket)
            var success = false
            try { client.network = PhysicalNetwork.choose(getSystemService(ConnectivityManager::class.java)); action(client); success = true; ui { if (gate.accepts(ticket)) { state.text = "$stage 完成"; Diagnostics.event("$stage 完成") } } }
            catch (e: Exception) { ui { if (gate.accepts(ticket)) { state.text = "$stage：${Diagnostics.error(e)}"; updateText.text = state.text; Diagnostics.event("$stage 失败：${e.javaClass.simpleName} ${Diagnostics.error(e)}") } } }
            finally { ui { if (gate.accepts(ticket)) { busy = false; operationApi = null; updater = null; if (success && (stage == "密码登录" || stage == "飞书登录")) { reloadAccount(); val saved = api.saved(); if (!saved.optBoolean("mustChange")) checkUpdates(saved.getString("server")) } } }; taskEpoch.remove() }
        }
    }
    private fun cancelOperation() { gate.cancel(); operationApi?.cancel(); updater?.cancel(); operation?.cancel(true); busy = false; pendingInstall = false; actionAfterStop = null; state.text = "操作已取消"; Diagnostics.event("用户取消操作") }
    private fun stopTunnel() { startService(Intent(this, TunnelService::class.java).setAction(TunnelService.STOP)) }
    private fun afterDisconnect(action: () -> Unit) {
        if (!TunnelService.running) { action(); return }
        AlertDialog.Builder(this).setTitle("先断开 VPN").setMessage("此操作需要先停止当前连接。")
            .setNegativeButton("取消", null).setPositiveButton("断开并继续") { _, _ -> actionAfterStop = action; stopTunnel() }.show()
    }
    private fun install() {
        val file = ready ?: return; val update = candidate ?: return
        if (runCatching { Api.validServer(server.text.toString()) }.getOrNull() != update.server) { state.text = "服务器已变更，请重新检查更新"; return }
        if (!packageManager.canRequestPackageInstalls()) {
            pendingInstall = true
            try { startActivity(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:$packageName"))) } catch (_: Exception) { pendingInstall = false; state.text = "请在系统设置允许安装此来源的应用" }
            return
        }
        work("安装前校验") {
            val verifier = ClientUpdates(this)
            updater = verifier
            verifier.verifyPackage(file, update)
            ui { if (!TunnelService.running) {
                try { startActivity(Intent(Intent.ACTION_VIEW).setDataAndType(Uri.parse("content://$packageName.updates/${file.name}"), "application/vnd.android.package-archive").addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)); Diagnostics.event("已交给系统安装器") }
                catch (_: Exception) { state.text = "无法打开系统安装器" }
            } }
        }
    }
    private fun passwordDialog() {
        val fields = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(40, 12, 40, 12) }
        fun field(hint: String) = EditText(this).apply { this.hint = hint; inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD; isSaveEnabled = false; fields.addView(this) }
        val old = field("当前密码"); val new = field("新密码"); val confirm = field("确认新密码")
        val dialog = AlertDialog.Builder(this).setTitle("修改密码").setView(fields).setNegativeButton("取消", null).setPositiveButton("保存", null).create()
        dialog.setOnShowListener { dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener {
            if (busy || TunnelService.running) { confirm.error = "请等待当前操作完成后重试"; return@setOnClickListener }
            if (new.text.toString() != confirm.text.toString() || new.text.isEmpty()) { confirm.error = "两次新密码须一致且不能为空"; return@setOnClickListener }
            if (!validPassword(new.text.toString())) { new.error = "新密码至少8位，包含字母和数字"; return@setOnClickListener }
            val oldSecret = old.text.toString(); val newSecret = new.text.toString(); old.text.clear(); new.text.clear(); confirm.text.clear(); dialog.dismiss()
            work("修改密码") { client -> client.changePassword(oldSecret, newSecret); ui { account.text = "密码已修改，请重新登录" } }
        } }; dialog.show()
    }
    companion object { fun validPassword(value: String) = value.length >= 8 && value.any { it.isLetter() } && value.any { it.isDigit() } }
    private fun connect() {
        if (busy || TunnelService.running) return
        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != android.content.pm.PackageManager.PERMISSION_GRANTED) requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 11)
        try { TunnelService.requestStart(this) } catch (e: Exception) { state.text = Diagnostics.error(e) }
    }
    @Deprecated("Legacy activity result supports API 26")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) { super.onActivityResult(requestCode, resultCode, data); if (requestCode == 10) { val ticket = vpnPermissionTicket; vpnPermissionTicket = null; if (resultCode == RESULT_OK && ticket != null && gate.accepts(ticket) && !busy && reloadAccount() && api.saved().has("refresh")) connect() else state.text = "VPN 授权取消或账号状态已变更，请重新连接" } }
    override fun onResume() { super.onResume(); if (::api.isInitialized && !busy && !TunnelService.running) reloadAccount(); if (pendingInstall) { pendingInstall = false; if (packageManager.canRequestPackageInstalls()) afterDisconnect { install() } else state.text = "未授予安装权限，当前应用可继续使用" } }
    override fun onDestroy() { destroyed = true; handler.removeCallbacksAndMessages(null); if (::api.isInitialized) cancelOperation(); executor.shutdownNow(); super.onDestroy() }
}
