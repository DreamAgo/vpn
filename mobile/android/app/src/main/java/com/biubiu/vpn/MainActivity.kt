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
    private lateinit var activityEvents: LinearLayout
    private var lastActivities: String? = null
    private var updateCheckButton: Button? = null
    private var updateDownloadButton: Button? = null
    private var updateInstallButton: Button? = null
    private var updateCancelButton: Button? = null
    private var updateOperation = false
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
    private lateinit var design: NativeUi
    private lateinit var shell: LinearLayout
    private lateinit var pages: ViewFlipper
    private lateinit var loginPage: ScrollView
    private lateinit var navigation: LinearLayout
    private lateinit var title: TextView
    private lateinit var subtitle: TextView
    private lateinit var metricsCard: LinearLayout
    private lateinit var privacyCard: TextView
    private var compactHome = false
    private lateinit var duration: TextView
    private lateinit var upload: TextView
    private lateinit var download: TextView
    private lateinit var primary: Button
    private lateinit var power: ImageButton
    private lateinit var cancelButton: Button
    private lateinit var errorCard: LinearLayout
    private lateinit var errorText: TextView
    private lateinit var workspace: TextView
    private lateinit var serverRow: Button
    private lateinit var updateRow: Button
    private var sessionInitialized = false
    private var sessionVisible = false
    private var sessionIdentity = ""
    private var sheetDialog: Dialog? = null
    private val tabButtons = mutableListOf<Button>()
    private val refresh = object : Runnable {
        override fun run() {
            if (!busy && !TunnelService.running) reloadAccount()
            state.visibility = if (state.text.isNotBlank() && (busy || !sessionVisible)) android.view.View.VISIBLE else android.view.View.GONE
            refreshConnection()
            server.isEnabled = !busy && !TunnelService.running
            username.isEnabled = !busy; password.isEnabled = !busy
            mutableButtons.forEach { it.isEnabled = !busy && !TunnelService.running; it.alpha = if (it.isEnabled) 1f else .45f }
            cancelButton.visibility = if (busy || actionAfterStop != null) android.view.View.VISIBLE else android.view.View.GONE
            if (!TunnelService.running && actionAfterStop != null) { val next = actionAfterStop; actionAfterStop = null; next?.invoke() }
            if (visiblePage == 1) refreshActivities()
            refreshUpdateActions()
            updateRow.text = if (candidate != null) "应用更新 · 有新版本" else "应用更新"
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
        design = NativeUi(this)
        visiblePage = savedInstanceState?.getInt("selectedTab", 0)?.coerceIn(0, 2) ?: 0
        shell = design.column().apply { setBackgroundColor(design.background) }
        state = design.text(shell, "", 12f, design.muted).apply { maxLines = 1; ellipsize = android.text.TextUtils.TruncateAt.END; setOnClickListener { sheet("操作提示") { box -> design.text(box, text.toString(), 14f) } }; setPadding(design.dp(24), design.dp(4), design.dp(24), design.dp(4)); accessibilityLiveRegion = android.view.View.ACCESSIBILITY_LIVE_REGION_POLITE }
        pages = ViewFlipper(this); shell.addView(pages, LinearLayout.LayoutParams(-1, 0, 1f))
        fun page(): LinearLayout = design.column(24).also { content -> pages.addView(ScrollView(this).apply { isFillViewport = true; isVerticalScrollBarEnabled = false; addView(content) }) }
        // The connection screen is a fixed viewport, not a scrollable document.
        val connectPage = design.column().apply { setPadding(design.dp(24), design.dp(8), design.dp(24), design.dp(8)) }
        pages.addView(connectPage, android.widget.FrameLayout.LayoutParams(-1, -1))
        val space = design.card(connectPage).apply { setPadding(design.dp(14), design.dp(8), design.dp(14), design.dp(8)) }
        workspace = design.text(space, "工作网络", 13f, bold = true).apply {
            maxLines = 1; ellipsize = android.text.TextUtils.TruncateAt.END
            setCompoundDrawablesWithIntrinsicBounds(NativeLineIcon(design.dp(20), design.blue, 5), null, NativeLineIcon(design.dp(16), design.muted, 3), null)
            compoundDrawablePadding = design.dp(10); minHeight = design.dp(32)
        }
        space.setOnClickListener { detailsSheet() }; space.contentDescription = "查看工作网络连接详情"; space.isFocusable = true
        val hero = ConnectionHeroLayout(this, design)
        connectPage.addView(hero, LinearLayout.LayoutParams(-1, 0, 1f))
        val ring = FrameLayout(this).apply { background = design.shape(design.soft, 100, true) }
        power = ImageButton(this).apply {
            setImageDrawable(NativeLineIcon(design.dp(40), design.blue, 0)); scaleType = ImageView.ScaleType.CENTER_INSIDE
            setPadding(design.dp(20), design.dp(20), design.dp(20), design.dp(20)); background = design.shape(android.graphics.Color.WHITE, 100, true)
            contentDescription = "连接工作网络"; setOnClickListener { connectionAction() }
        }
        ring.addView(android.view.View(this).apply { background = design.shape(design.soft, 100, true) }, FrameLayout.LayoutParams(design.dp(144), design.dp(144), android.view.Gravity.CENTER))
        ring.addView(power, FrameLayout.LayoutParams(design.dp(116), design.dp(116), android.view.Gravity.CENTER))
        hero.addView(ring, LinearLayout.LayoutParams(design.dp(168), design.dp(168)).apply { bottomMargin = design.dp(12) })
        title = design.text(hero, "准备连接", 24f, bold = true).also(design::centered).apply {
            maxLines = 1; setAutoSizeTextTypeUniformWithConfiguration(16, 24, 1, android.util.TypedValue.COMPLEX_UNIT_SP)
        }
        subtitle = design.text(hero, "安全访问你的工作网络", 12f, design.muted).also(design::centered).apply {
            maxLines = 1; ellipsize = android.text.TextUtils.TruncateAt.END
            setOnClickListener { detailsSheet() }
        }
        primary = design.button(connectPage, "连接工作网络", true) { connectionAction() }
        val metrics = design.card(connectPage).apply { setPadding(design.dp(8), design.dp(4), design.dp(8), design.dp(4)) }; metricsCard = metrics
        val row = LinearLayout(this); metrics.addView(row)
        fun metric(label: String): TextView { val col = design.column(); row.addView(col, LinearLayout.LayoutParams(0, -2, 1f)); design.text(col, label, 10f, design.muted).also(design::centered).setPadding(0, 0, 0, 0); return design.text(col, "—", 13f, bold = true).also(design::centered).apply { maxLines = 1; setPadding(0, design.dp(2), 0, 0) } }
        duration = metric("连接时长"); upload = metric("已上传"); download = metric("已下载")
        errorCard = design.card(connectPage, android.graphics.Color.rgb(255, 244, 237)).apply { setPadding(design.dp(12), design.dp(4), design.dp(12), design.dp(4)) }
        errorText = design.text(errorCard, "", 12f, design.muted).apply { maxLines = 2; ellipsize = android.text.TextUtils.TruncateAt.END }
        errorCard.setOnClickListener { diagnosticsSheet() }; errorCard.contentDescription = "查看完整错误与处理建议"; errorCard.isFocusable = true
        privacyCard = design.text(connectPage, "仅工作网络通过安全通道，日常上网保持直连。", 12f, design.blue).apply {
            background = design.shape(design.soft, 14); setPadding(design.dp(14), design.dp(12), design.dp(14), design.dp(12)); maxLines = 2
        }
        connectPage.addOnLayoutChangeListener { _, _, top, _, bottom, _, _, _, _ ->
            val compact = bottom - top < design.dp(380)
            if (compact != compactHome) { compactHome = compact; refreshConnection() }
        }
        detail = TextView(this)
        val activityPage = page()
        design.text(activityPage, "活动", 25f, bold = true)
        design.text(activityPage, "连接与操作记录，帮助你了解当前状态。", 12f, design.muted)
        design.text(activityPage, "最近活动", 12f, design.muted)
        activityEvents = design.column(); activityPage.addView(activityEvents)
        refreshActivities()
        design.button(activityPage, "复制诊断") { copyDiagnostics() }
        design.button(activityPage, "清空记录") { Diagnostics.clear(); refreshActivities() }
        val mine = page()
        design.text(mine, "我的", 25f, bold = true)
        val profile = design.card(mine)
        account = design.text(profile, "", 22f, bold = true)
        design.text(profile, "已登录工作账号", 12f, design.muted)
        design.text(mine, "工作空间", 12f, design.muted)
        serverRow = design.button(mine, "服务地址") { serverSheet() }
        design.button(mine, "修改密码") { if (!busy) afterDisconnect { passwordDialog() } }
        design.gap(mine)
        design.text(mine, "应用与支持", 12f, design.muted)
        updateRow = design.button(mine, "应用更新") { updateSheet() }
        design.button(mine, "诊断与日志") { diagnosticsSheet() }
        design.button(mine, "系统 VPN 设置") { runCatching { startActivity(Intent(Settings.ACTION_VPN_SETTINGS)) }.onFailure { state.text = "无法打开系统 VPN 设置" } }
        design.button(mine, "关于易链") { sheet("关于易链") { box -> design.text(box, "易链 · ${BuildConfig.VERSION_NAME}", 22f, bold = true); design.text(box, "单屏界面 · 构建 ${BuildConfig.UI_BUILD}", 12f, design.muted); design.text(box, "安全访问工作网络。\nAndroid ${Build.VERSION.RELEASE}\n仅授权网段使用 VPN，连接以握手为准。\n暂不支持开机自动连接或始终开启 VPN。", 13f, design.muted) } }
        design.button(mine, "退出登录") { if (!busy) afterDisconnect { work("退出登录") { client -> client.logout(); ui { reloadAccount() } } } }.setTextColor(android.graphics.Color.rgb(170, 70, 60))
        design.text(mine, "易链 ${BuildConfig.VERSION_NAME} · 单屏 ${BuildConfig.UI_BUILD}", 11f, design.muted).also(design::centered)
        val login = design.column(24)
        loginPage = ScrollView(this).apply { isFillViewport = true; isVerticalScrollBarEnabled = false; addView(login) }; shell.addView(loginPage, LinearLayout.LayoutParams(-1, 0, 1f))
        design.text(login, "连接工作，\n也连接安心。", 30f, bold = true)
        design.text(login, "登录你的工作账号，安全访问内部资源。", 13f, design.muted)
        design.gap(login, 16)
        design.text(login, "服务地址", 12f, design.muted)
        server = design.field(login, "https://vpn.example.com", InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_URI)
        mutableButtons.add(design.button(login, "使用飞书登录", true) { feishuLogin() })
        design.text(login, "或使用账号密码", 12f, design.muted).also(design::centered)
        username = design.field(login, "账号", InputType.TYPE_CLASS_TEXT)
        password = design.field(login, "密码", InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD).apply { isSaveEnabled = false; importantForAutofill = android.view.View.IMPORTANT_FOR_AUTOFILL_NO }
        mutableButtons.add(design.button(login, "密码登录") {
            val host = server.text.toString(); val user = username.text.toString(); val secret = password.text.toString(); password.text.clear()
            work("密码登录") { client -> val change = client.login(host, user, secret); ui { reloadAccount(); if (change) passwordDialog() } }
        })
        design.text(login, "授权完成后返回易链。首次连接需允许系统 VPN 请求。", 12f, design.muted)
        design.button(login, "登录遇到问题？查看诊断") { diagnosticsSheet() }
        design.button(login, "检查应用更新") { updateSheet() }
        cancelButton = design.button(shell, "取消当前操作") { cancelOperation() }.apply { visibility = android.view.View.GONE }
        navigation = LinearLayout(this).apply { setPadding(design.dp(12), design.dp(8), design.dp(12), design.dp(8)); setBackgroundColor(android.graphics.Color.WHITE) }
        listOf("连接", "活动", "我的").forEachIndexed { index, label ->
            val button = Button(this).apply {
                stateListAnimator = null; elevation = 0f; translationZ = 0f; outlineProvider = null
                backgroundTintList = null
                background = android.graphics.drawable.RippleDrawable(android.content.res.ColorStateList.valueOf(0x142563EB), null, null)
                setPadding(design.dp(8), design.dp(6), design.dp(8), design.dp(6))
                text = label; setCompoundDrawablesWithIntrinsicBounds(null, NativeLineIcon(design.dp(22), design.muted, if (index == 0) 4 else index), null, null); compoundDrawablePadding = design.dp(4); textSize = 12f; isAllCaps = false; minHeight = design.dp(52); setOnClickListener { selectTab(index) } }
            navigation.addView(button, LinearLayout.LayoutParams(0, -2, 1f)); tabButtons.add(button)
        }
        shell.addView(navigation)
        updateText = TextView(this).apply { text = "从当前工作服务器获取更新。"; textSize = 14f; setTextColor(design.muted); setPadding(0, design.dp(12), 0, design.dp(12)) }
        val saved = api.saved(); server.setText(saved.optString("server")); username.setText(saved.optString("username"))
        setContentView(shell)
        shell.setOnApplyWindowInsetsListener { view, insets -> view.setPadding(insets.systemWindowInsetLeft, insets.systemWindowInsetTop, insets.systemWindowInsetRight, insets.systemWindowInsetBottom); insets }
        reloadAccount(); selectTab(visiblePage); handler.post(refresh)
        if (saved.has("refresh") && !saved.optBoolean("mustChange")) handler.post { checkUpdates(saved.getString("server")) }
    }
    private fun selectTab(index: Int) {
        visiblePage = index; pages.displayedChild = index
        tabButtons.forEachIndexed { i, button -> button.setTextColor(if (i == index) design.blue else design.muted); button.isSelected = i == index; button.setCompoundDrawablesWithIntrinsicBounds(null, NativeLineIcon(design.dp(22), if (i == index) design.blue else design.muted, if (i == 0) 4 else i), null, null) }
        if (index == 1) refreshActivities()
    }
    private fun refreshActivities() {
        val logs = Diagnostics.snapshot()
        if (logs == lastActivities) return
        lastActivities = logs; activityEvents.removeAllViews()
        if (logs.isBlank()) {
            val card = design.card(activityEvents)
            design.text(card, "还没有连接记录", 14f, bold = true)
            design.text(card, "连接工作网络后，成功、重连与异常都会出现在这里。", 12f, design.muted)
            return
        }
        logs.lines().asReversed().forEach { entry ->
            val row = LinearLayout(this).apply { gravity = android.view.Gravity.TOP; setPadding(0, design.dp(10), 0, design.dp(10)) }
            val glyph = ImageView(this).apply { setImageDrawable(NativeLineIcon(design.dp(18), design.blue, 1)); setPadding(design.dp(7), design.dp(7), design.dp(7), design.dp(7)); background = design.shape(design.soft, 20); importantForAccessibility = android.view.View.IMPORTANT_FOR_ACCESSIBILITY_NO }
            row.addView(glyph, LinearLayout.LayoutParams(design.dp(32), design.dp(32)))
            val content = design.column().apply { setPadding(design.dp(12), 0, 0, 0) }
            row.addView(content, LinearLayout.LayoutParams(0, -2, 1f))
            design.text(content, entry.substringAfter(' ', entry), 13f).setTextIsSelectable(true)
            design.text(content, entry.substringBefore(' '), 11f, design.muted)
            activityEvents.addView(row)
        }
    }
    private fun refreshUpdateActions() {
        listOf(updateCheckButton to !busy, updateDownloadButton to (!busy && candidate != null), updateInstallButton to (!busy && ready != null), updateCancelButton to (busy && updateOperation)).forEach { (button, enabled) ->
            button?.isEnabled = enabled; button?.alpha = if (enabled) 1f else .45f
        }
    }
    private fun refreshConnection() {
        val model = ConnectionPresentation.from(TunnelService.running, TunnelService.status, TunnelService.details)
        title.text = model.title; subtitle.text = if (model.failed) "点击下方提示查看原因与处理建议" else model.subtitle; primary.text = model.actionLabel
        primary.isEnabled = (!busy || TunnelService.running) && !model.stopping && actionAfterStop == null; power.isEnabled = primary.isEnabled
        power.contentDescription = model.actionLabel
        primary.alpha = if (primary.isEnabled) 1f else .45f; power.alpha = primary.alpha
        power.setImageDrawable(NativeLineIcon(design.dp(40), if (model.connected) android.graphics.Color.WHITE else design.blue, if (model.connected) 4 else 0))
        power.background = design.shape(if (model.connected) design.blue else android.graphics.Color.WHITE, 100, true)
        metricsCard.visibility = if (model.connected && !compactHome) android.view.View.VISIBLE else android.view.View.GONE
        duration.text = model.duration; upload.text = model.uploaded; download.text = model.downloaded
        errorCard.visibility = if (model.failed) android.view.View.VISIBLE else android.view.View.GONE
        errorText.text = "${model.subtitle} · 点击查看详情"
        privacyCard.visibility = if (!model.connected && !model.failed && !compactHome) android.view.View.VISIBLE else android.view.View.GONE
        detail.text = "服务器：$connectedServer\n${TunnelService.status}\n${TrafficFormat.details(TunnelService.details)}"
    }
    private fun connectionAction() {
        if (actionAfterStop != null) return
        if (TunnelService.running) {
            val model = ConnectionPresentation.from(true, TunnelService.status, TunnelService.details)
            if (model.connected) AlertDialog.Builder(this).setTitle("断开工作网络？").setMessage("断开后将无法通过易链访问内部资源。").setNegativeButton("保持连接", null).setPositiveButton("断开连接") { _, _ -> stopTunnel() }.show()
            else stopTunnel()
            return
        }
        if (busy) return
        if (!reloadAccount() || !api.saved().has("refresh")) { state.text = "请先登录工作账号"; return }
        if (api.saved().optBoolean("mustChange")) { passwordDialog(); return }
        val permission = VpnService.prepare(this)
        if (permission != null) { vpnPermissionTicket = gate.current(); startActivityForResult(permission, 10) } else connect()
    }
    private fun feishuLogin() {
        val host = server.text.toString()
        work("飞书登录") { client -> FeishuLogin(client).run(host) { url ->
            val latch = java.util.concurrent.CountDownLatch(1); var failure: Exception? = null
            ui { try { startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addCategory(Intent.CATEGORY_BROWSABLE)); state.text = "在浏览器授权后返回，可点击取消当前操作" } catch (e: Exception) { failure = e } finally { latch.countDown() } }
            check(latch.await(5, java.util.concurrent.TimeUnit.SECONDS)); failure?.let { throw it }
        }; ui { reloadAccount(); if (api.saved().optBoolean("mustChange")) passwordDialog() } }
    }
    private fun sheet(heading: String, content: (LinearLayout) -> Unit) {
        sheetDialog?.dismiss()
        val dialog = Dialog(this); val box = design.column(24).apply { background = design.shape(android.graphics.Color.WHITE, 24) }
        design.text(box, heading, 23f, bold = true); content(box)
        design.button(box, "关闭") { dialog.dismiss() }
        dialog.setContentView(ScrollView(this).apply { isVerticalScrollBarEnabled = false; addView(box) })
        dialog.window?.apply { setBackgroundDrawableResource(android.R.color.transparent); setGravity(android.view.Gravity.BOTTOM); addFlags(android.view.WindowManager.LayoutParams.FLAG_SECURE); setSoftInputMode(android.view.WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE) }
        dialog.show(); dialog.window?.setLayout(-1, (resources.displayMetrics.heightPixels * .8).toInt())
        sheetDialog = dialog
    }
    private fun detailsSheet() { sheet("连接详情") { box ->
        design.text(box, "${if (ConnectionPresentation.from(TunnelService.running, TunnelService.status, TunnelService.details).connected) "当前连接" else "最近连接快照（非实时）"}\n服务器：$connectedServer\n${TunnelService.status}\n${TrafficFormat.details(TunnelService.details)}", 13f, design.muted).setTextIsSelectable(true)
        design.button(box, "复制连接详情") { copy("易链连接", "$connectedServer\n${TrafficFormat.details(TunnelService.details)}"); Toast.makeText(this, "连接详情已复制", Toast.LENGTH_SHORT).show() }
    } }
    private fun copy(title: String, value: String) { getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText(title, value)) }
    private fun copyDiagnostics() {
        copy("易链诊断", "易链 ${BuildConfig.VERSION_NAME}\nAndroid ${Build.VERSION.RELEASE}\n${TunnelService.status}\n${TrafficFormat.details(TunnelService.details)}\n${Diagnostics.snapshot()}")
        Toast.makeText(this, "诊断已复制（包含内网地址）", Toast.LENGTH_SHORT).show()
    }
    private fun diagnosticsSheet() { sheet("诊断与支持") { box ->
        design.text(box, "${TunnelService.status}\n${TrafficFormat.details(TunnelService.details)}", 13f, design.muted).setTextIsSelectable(true)
        design.text(box, "请先检查当前网络与服务地址。若服务端拒绝请求，请将错误信息交给管理员确认；错误码本身不能确定原因。", 13f, design.muted)
        design.button(box, "复制诊断") { copyDiagnostics() }
        design.text(box, Diagnostics.snapshot().ifBlank { "暂无活动记录。" }, 12f, design.muted).setTextIsSelectable(true)
        if (sessionVisible) design.button(box, "查看活动记录") { sheetDialog?.dismiss(); selectTab(1) }
    } }
    private fun serverSheet() { sheet("服务地址") { box ->
        design.text(box, connectedServer, 16f, bold = true)
        design.text(box, "更换服务需要退出当前账号，并在登录页填写新的 HTTPS 地址。", 13f, design.muted)
        design.button(box, "更换服务并重新登录") { if (!busy) afterDisconnect { work("退出登录") { client -> client.logout(); ui { candidate = null; ready = null; reloadAccount(); server.text.clear(); sheetDialog?.dismiss() } } } }
    } }
    private fun updateSheet() { sheet("应用更新") { box ->
        design.text(box, "当前版本 ${BuildConfig.VERSION_NAME}", 14f, bold = true)
        (updateText.parent as? android.view.ViewGroup)?.removeView(updateText); box.addView(updateText)
        updateCheckButton = design.button(box, "检查更新", true) { checkUpdates(server.text.toString()) }
        updateDownloadButton = design.button(box, "下载并校验更新") { downloadUpdate() }
        updateInstallButton = design.button(box, "安装已校验的更新") { if (!busy && ready != null) afterDisconnect { install() } else if (!busy) updateText.text = "请先下载并校验更新" }
        updateCancelButton = design.button(box, "取消更新操作") { if (busy && updateOperation) { cancelOperation(); updateText.text = "更新操作已取消" } }
        refreshUpdateActions()
    } }
    private fun downloadUpdate() {
        val update = candidate ?: run { updateText.text = "请先检查是否有可用更新"; return }
        if (busy) return
        AlertDialog.Builder(this).setTitle("下载 ${update.version}")
            .setMessage("安装包 ${update.size / 1024} KiB。下载后会验证版本、完整性与签名，再由系统确认安装。")
            .setNegativeButton("取消", null).setPositiveButton("下载") { _, _ -> startDownload(update) }.show()
    }
    private fun startDownload(update: UpdatePackage) {
        if (busy || candidate != update) return
        val host = server.text.toString()
        work("下载与校验", true) {
            require(Api.validServer(host) == update.server)
            val client = ClientUpdates(this, PhysicalNetwork.choose(getSystemService(ConnectivityManager::class.java)) ?: error("没有网络")); updater = client
            var lastPercent = -1
            val file = client.download(update) { done, total -> val percent = (done * 100 / total).toInt(); if (percent != lastPercent) { lastPercent = percent; ui { updateText.text = if (percent == 100) "下载完成，正在校验完整性与签名…" else "下载中 $percent%" } } }
            ui { ready = file; updateText.text = "${update.version} 校验通过，可交给系统安装。安装前将断开 VPN。" }
        }
    }
    override fun onSaveInstanceState(outState: Bundle) { outState.putInt("selectedTab", visiblePage); super.onSaveInstanceState(outState) }
    private fun checkUpdates(host: String) {
        work("更新检查", allowConnected = true) {
            val client = ClientUpdates(this, PhysicalNetwork.choose(getSystemService(ConnectivityManager::class.java)) ?: throw LocalFailure("没有可用网络")); updater = client
            val update = client.check(host)
            ui { candidate = update; ready = null; updateText.text = if (update == null) "当前已是最新版本" else "可用版本 ${update.version}\n${update.size / 1024} KiB\n${update.notes}" }
        }
    }
    private fun reloadAccount(): Boolean {
        if (!TunnelService.running) try { api = Api(Vault(this)) } catch (_: Exception) { state.text = "安全凭据不可读取"; return false }
        val saved = api.saved(); connectedServer = saved.optString("server")
        val authenticated = saved.has("refresh")
        val identity = saved.optString("username") + "|" + connectedServer
        if (authenticated != sessionVisible || identity != sessionIdentity) {
            if (authenticated) { server.setText(connectedServer); username.setText(saved.optString("username")); if (sessionInitialized && !sessionVisible) selectTab(0) }
            if (sessionInitialized && (identity != sessionIdentity || !authenticated)) { candidate = null; ready = null }
            if (sessionVisible && !authenticated && TunnelService.status.startsWith("连接停止")) state.text = TunnelService.status
            sessionVisible = authenticated; sessionIdentity = identity
            account.text = saved.optString("username").ifBlank { "工作账号" }
            workspace.text = connectedServer.ifBlank { "工作网络" }
        }
        sessionInitialized = true
        pages.visibility = if (authenticated) android.view.View.VISIBLE else android.view.View.GONE
        navigation.visibility = pages.visibility
        loginPage.visibility = if (authenticated) android.view.View.GONE else android.view.View.VISIBLE
        return !TunnelService.running
    }
    private fun work(stage: String, allowConnected: Boolean = false, action: (Api) -> Unit) {
        if (busy || (!allowConnected && TunnelService.running)) return
        if (!allowConnected && !reloadAccount()) return
        busy = true
        updateOperation = stage in setOf("更新检查", "下载与校验", "安装前校验")
        if (updateOperation) updateText.text = "$stage…"
        refreshUpdateActions()
        val ticket = gate.begin(); state.text = "$stage…"; Diagnostics.event("$stage 开始")
        val client = api
        operationApi = client
        operation = executor.submit {
            taskEpoch.set(ticket)
            var success = false
            try { client.network = PhysicalNetwork.choose(getSystemService(ConnectivityManager::class.java)); action(client); success = true; ui { if (gate.accepts(ticket)) { state.text = "$stage 完成"; Diagnostics.event("$stage 完成") } } }
            catch (e: Exception) { ui { if (gate.accepts(ticket)) { state.text = "$stage：${Diagnostics.error(e)}"; updateText.text = state.text; Diagnostics.event("$stage 失败：${e.javaClass.simpleName} ${Diagnostics.logError(e)}") } } }
            finally { ui { if (gate.accepts(ticket)) { busy = false; updateOperation = false; refreshUpdateActions(); operationApi = null; updater = null; if (success && (stage == "密码登录" || stage == "飞书登录")) { reloadAccount(); val saved = api.saved(); if (!saved.optBoolean("mustChange")) checkUpdates(saved.getString("server")) } } }; taskEpoch.remove() }
        }
    }
    private fun cancelOperation() { if (updateOperation) updateText.text = "更新操作已取消，可重新检查或继续安装已校验的更新"; gate.cancel(); operationApi?.cancel(); updater?.cancel(); operation?.cancel(true); busy = false; updateOperation = false; refreshUpdateActions(); pendingInstall = false; actionAfterStop = null; state.text = "操作已取消"; Diagnostics.event("用户取消操作") }
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
                try { startActivity(Intent(Intent.ACTION_VIEW).setDataAndType(Uri.parse("content://$packageName.updates/${file.name}"), "application/vnd.android.package-archive").addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)); Diagnostics.event("已交给系统安装器"); updateText.text = "校验通过，已交给系统安装器。若取消安装，可再次点击安装。" }
                catch (_: Exception) { state.text = "无法打开系统安装器"; updateText.text = state.text }
            } }
        }
    }
    private fun passwordDialog() {
        val fields = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL; setPadding(design.dp(24), design.dp(12), design.dp(24), design.dp(12)) }
        fun field(hint: String) = EditText(this).apply { this.hint = hint; inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_VARIATION_PASSWORD; isSaveEnabled = false; fields.addView(this) }
        val old = field("当前密码"); val new = field("新密码"); val confirm = field("确认新密码")
        val dialog = AlertDialog.Builder(this).setTitle("修改密码").setView(fields).setNegativeButton("取消", null).setPositiveButton("保存", null).create()
        dialog.setOnShowListener { dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener {
            if (busy || TunnelService.running) { confirm.error = "请等待当前操作完成后重试"; return@setOnClickListener }
            if (new.text.toString() != confirm.text.toString() || new.text.isEmpty()) { confirm.error = "两次新密码须一致且不能为空"; return@setOnClickListener }
            if (!validPassword(new.text.toString())) { new.error = "新密码至少8位，包含字母和数字"; return@setOnClickListener }
            val oldSecret = old.text.toString(); val newSecret = new.text.toString(); old.text.clear(); new.text.clear(); confirm.text.clear(); dialog.dismiss()
            work("修改密码") { client -> client.changePassword(oldSecret, newSecret); ui { reloadAccount(); state.text = "密码已修改，请重新登录" } }
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
    override fun onDestroy() { destroyed = true; sheetDialog?.dismiss(); handler.removeCallbacksAndMessages(null); if (::api.isInitialized) cancelOperation(); executor.shutdownNow(); super.onDestroy() }
}
