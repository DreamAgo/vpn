package com.biubiu.vpn

import android.app.*
import android.content.Intent
import android.net.*
import android.os.*
import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import org.json.JSONArray
import org.json.JSONObject
import java.net.*
import java.nio.ByteBuffer
import java.util.concurrent.Executors
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import kotlin.random.Random

class TunnelService : VpnService() {
    companion object {
        @Volatile var status = "未连接"
        @Volatile var details = "暂无连接详情"
        private val lifetime = TunnelLifetime()
        val running: Boolean get() = lifetime.running()
        const val STOP = "com.biubiu.vpn.STOP"
        private const val GENERATION = "generation"
        private const val QUALITY_SAMPLE_INTERVAL_MS = 2000L
        private const val QUALITY_SAMPLE_BUDGET_MS = 1500L
        private const val QUALITY_MAX_POLICY_AGE_MS = 45000L
        private const val QUALITY_MAX_SAMPLE_AGE_MS = 5000L
        private const val QUALITY_REPORT_INTERVAL_MS = 15000L
        private const val QUALITY_SWITCH_COOLDOWN_MS = 60000L
        private const val QUALITY_MAX_PROBE_WORKERS = 16
        fun requestStart(context: android.content.Context) {
            val token = lifetime.begin() ?: return
            try { context.startForegroundService(Intent(context, TunnelService::class.java).putExtra(GENERATION, token)) }
            catch (e: Exception) { lifetime.finish(token); throw e }
        }
    }
    private val stopping = AtomicBoolean(false)
    private var generation = -1L
    private var handshaken = false
    @Volatile private var worker: Thread? = null
    @Volatile private var socket: DatagramSocket? = null
    @Volatile private var tun: ParcelFileDescriptor? = null
    @Volatile private var api: Api? = null
    private lateinit var cm: ConnectivityManager
    override fun onCreate() { super.onCreate(); cm = getSystemService(ConnectivityManager::class.java) }
    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == STOP) { shutdown(); return START_NOT_STICKY }
        if (worker != null) return START_NOT_STICKY
        generation = intent?.getLongExtra(GENERATION, -1L) ?: -1L
        if (!lifetime.accepts(generation)) { stopSelf(startId); return START_NOT_STICKY }
        stopping.set(false)
        details = "等待本次连接信息"
        try {
            getSystemService(NotificationManager::class.java).createNotificationChannel(NotificationChannel("vpn", "VPN 连接", NotificationManager.IMPORTANCE_LOW))
            startForeground(1, notification("正在连接"))
            worker = Thread({ runConnection() }, "vpn-control").also { it.start() }
        } catch (e: Exception) {
            worker = null; lifetime.finish(generation)
            status = "启动失败：${Diagnostics.error(e)}"
            Diagnostics.event(status); stopForeground(STOP_FOREGROUND_REMOVE); stopSelf(startId)
        }
        return START_NOT_STICKY
    }
    private fun notification(text: String): Notification {
        val stop = PendingIntent.getService(this, 1, Intent(this, TunnelService::class.java).setAction(STOP), PendingIntent.FLAG_IMMUTABLE)
        val open = PendingIntent.getActivity(this, 2, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        return Notification.Builder(this, "vpn").setSmallIcon(R.drawable.ic_vpn_notification).setOnlyAlertOnce(true).setContentTitle("易链")
            .setContentText(text).setContentIntent(open).setOngoing(true).addAction(Notification.Action.Builder(null, "断开", stop).build()).build()
    }
    private fun report(text: String, diagnostic: String = text) {
        if (stopping.get() || !lifetime.accepts(generation)) return
        if (text.startsWith("已连接")) {
            if (!status.startsWith("已连接")) Diagnostics.event("WireGuard 握手成功，VPN 已连接")
        } else if (status != text) Diagnostics.event(diagnostic)
        status = text
        getSystemService(NotificationManager::class.java).notify(1, notification(text))
    }
    private fun closeIO() {
        socket?.close(); socket = null
        try { tun?.close() } catch (_: Exception) {} finally { tun = null }
    }
    private fun shutdown() {
        if (!stopping.get()) Diagnostics.event("请求断开 VPN")
        stopping.set(true); lifetime.stop(generation); api?.cancel(); closeIO(); worker?.interrupt()
        status = "正在断开"
        if (worker == null) { status = "已断开"; lifetime.finish(generation); stopForeground(STOP_FOREGROUND_REMOVE); stopSelf() }
    }
    override fun onRevoke() { shutdown(); super.onRevoke() }
    override fun onDestroy() { stopping.set(true); lifetime.stop(generation); api?.cancel(); closeIO(); worker?.interrupt(); if (worker == null) lifetime.finish(generation); super.onDestroy() }
    private class GatewayMove(val quality: QualityProposal? = null) :
        java.io.IOException("VPN 网关选择已更新")
    private class GatewayUnavailable(val gateway: GatewayChoice, reason: String) : java.io.IOException(reason)
    private data class QualityPolicy(val candidates: GatewayCandidates, val refreshedAt: Long)
    private data class QualityProposal(
        val move: GatewayQualityMove,
        val network: Network,
        val addresses: String,
        val sampledAt: Long,
    )
    private data class QualityObservation(
        val context: GatewayQualityContext,
        val stats: GatewayQualityStats,
        val addresses: String,
        val sampledAt: Long,
    )

    private fun runConnection() {
        var delay = 1000L
        val cluster = GatewayCluster { socket -> protect(socket) }
        var qualityPreference: QualityProposal? = null
        var qualityCooldownUntil = 0L
        try {
            while (!stopping.get()) {
                try {
                    val physical = PhysicalNetwork.choose(cm) ?: throw java.io.IOException("等待网络")
                    val client = Api(Vault(this)).also { it.network = physical; api = it }
                    report("正在注册设备")
                    client.ensureDeviceIdentity()
                    val preferred = qualityPreference?.takeIf {
                        physical == it.network && localAddresses(physical).toString() == it.addresses
                    }?.move
                    qualityPreference = null
                    val connection = cluster.prepare(client, physical, preferred)
                    check(!stopping.get())
                    handshaken = false
                    runTunnel(client, physical, connection, cluster, qualityCooldownUntil)
                    delay = 1000L
                } catch (e: Exception) {
                    if (stopping.get()) break
                    if (e is GatewayMove) {
                        qualityPreference = e.quality
                        if (e.quality != null) {
                            qualityCooldownUntil = SystemClock.elapsedRealtime() + QUALITY_SWITCH_COOLDOWN_MS
                        }
                        delay = 1000L
                        report("正在切换 VPN 网关")
                        continue
                    }
                    if (e is GatewayUnavailable) cluster.failed(e.gateway)
                    if (e is ApiError && e.stopsTunnel || e is IllegalArgumentException || e is IllegalStateException || e is org.json.JSONException) {
                        report("连接停止：${Diagnostics.error(e)}", "连接停止：${Diagnostics.logError(e)}"); break
                    }
                    if (handshaken) { delay = 1000L; handshaken = false }
                    report("连接中断，${delay / 1000} 秒后重试：${Diagnostics.error(e)}")
                    Thread.sleep(delay); delay = (delay * 2).coerceAtMost(30000)
                } finally { api?.cancel(); api = null; closeIO() }
            }
        } catch (_: InterruptedException) {
        } finally {
            closeIO(); worker = null
            Handler(Looper.getMainLooper()).post {
                if (lifetime.finish(generation)) { if (stopping.get()) { status = "已断开"; Diagnostics.event(status) }; stopForeground(STOP_FOREGROUND_REMOVE); stopSelf() }
            }
        }
    }
    private fun localAddresses(network: Network): JSONArray = JSONArray().apply {
        cm.getLinkProperties(network)?.linkAddresses?.forEach { if (it.address is Inet4Address) put(it.address.hostAddress) }
    }
    private fun runTunnel(
        client: Api,
        network: Network,
        connection: GatewayCluster.Connection,
        cluster: GatewayCluster,
        qualityCooldownUntil: Long,
    ) {
        val config = connection.config
        val currentGateway = AtomicReference(connection.gateway)
        val confirmedRevision = AtomicReference<Long?>(null)
        val dataPlaneReady = AtomicBoolean(false)
        var physicalAddresses = localAddresses(network)
        var plan = Native.request("plan", JSONObject().put("config", config).put("local", physicalAddresses))
        val endpoint = plan.getString("endpoint")
        val split = endpoint.lastIndexOf(':')
        val host = endpoint.substring(0, split)
        val address = try {
            network.getAllByName(host).firstOrNull { it is Inet4Address }
                ?: throw if (connection.gateway != null) UnknownHostException("未找到 IPv4 地址")
                    else IllegalArgumentException("首版需要 IPv4 VPN 服务端地址")
        } catch (e: UnknownHostException) {
            throw connection.gateway?.let { GatewayUnavailable(it, "无法解析 VPN 网关地址") } ?: e
        }
        val udp = DatagramSocket(null)
        socket = udp
        if (!protect(udp)) throw java.io.IOException("无法保护 VPN 外层 socket")
        network.bindSocket(udp)
        udp.bind(InetSocketAddress(0))
        try { udp.connect(address, endpoint.substring(split + 1).toInt()) }
        catch (e: SocketException) {
            throw connection.gateway?.let { GatewayUnavailable(it, "无法连接 VPN 网关 UDP 地址") } ?: e
        }
        udp.soTimeout = 1
        setUnderlyingNetworks(arrayOf(network))
        val handle = Native.request("create", JSONObject().put("config", config).put("private", client.saved().getString("private"))).getLong("id")
        val heartbeat = Executors.newSingleThreadExecutor()
        val heartbeatBusy = AtomicBoolean(false)
        val sampler = connection.gateway?.let { Executors.newSingleThreadExecutor() }
        val probeWorkers = connection.gateway?.let {
            ThreadPoolExecutor(1, QUALITY_MAX_PROBE_WORKERS, 10L, TimeUnit.SECONDS, LinkedBlockingQueue()).apply {
                allowCoreThreadTimeOut(true)
            }
        }
        val sampling = AtomicBoolean(false)
        val quality = GatewayQuality()
        val qualityPolicy = AtomicReference<QualityPolicy?>(null)
        val qualityProposal = AtomicReference<QualityProposal?>(null)
        val qualityObservation = AtomicReference<QualityObservation?>(null)
        val update = AtomicReference<JSONObject?>(null)
        val failure = AtomicReference<Exception?>(null)
        try {
            tun = establish(plan)
            check(!stopping.get())
            send(handle, 3, byteArrayOf(), udp)
            val buffer = ByteArray(65535)
            var lastNetworkCheck = 0L
            var lastTimer = 0L; var lastStatus = 0L; var lastHeartbeat = 0L; var lastConfirmation = 0L
            val started = SystemClock.elapsedRealtime()
            var nextSample = started + Random.nextLong(QUALITY_SAMPLE_INTERVAL_MS)
            var lastQualityReport = 0L
            val rateMeter = TrafficRateMeter(started)
            report("正在进行 WireGuard 握手")
            fun clearQualityPolicy() {
                qualityPolicy.set(null)
                qualityProposal.set(null)
                qualityObservation.set(null)
            }
            fun matchesPhysicalNetwork(addresses: String, sampledAt: Long): Boolean =
                !stopping.get() && SystemClock.elapsedRealtime() - sampledAt <= QUALITY_MAX_SAMPLE_AGE_MS &&
                    PhysicalNetwork.choose(cm, network) == network && localAddresses(network).toString() == addresses

            fun refreshGateway(current: GatewayChoice): GatewayChoice {
                val candidates = client.candidates()
                if (candidates == null) {
                    clearQualityPolicy()
                    return current
                }
                if (cluster.instructedSwitch(candidates, current)) throw GatewayMove()
                return current.copy(revision = candidates.revision).also {
                    currentGateway.set(it)
                    qualityPolicy.set(QualityPolicy(candidates, SystemClock.elapsedRealtime()))
                    val context = candidates.qualityContext(it)
                    qualityProposal.updateAndGet { proposal -> proposal?.takeIf { pending -> pending.move.context == context } }
                    qualityObservation.updateAndGet { observation -> observation?.takeIf { reading -> reading.context == context } }
                }
            }
            while (!stopping.get()) {
                failure.getAndSet(null)?.let { throw it }
                val newConfig = update.getAndSet(null)
                val checkNetwork = SystemClock.elapsedRealtime() - lastNetworkCheck >= 1000
                if (checkNetwork && PhysicalNetwork.choose(cm, network) != network) throw java.io.IOException("网络已切换")
                val local = if (checkNetwork) localAddresses(network) else physicalAddresses
                if (checkNetwork) lastNetworkCheck = SystemClock.elapsedRealtime()
                if (newConfig != null || local.toString() != physicalAddresses.toString()) {
                    if (newConfig != null) {
                        val routes = newConfig.optJSONArray("allowed_routes")
                        if (routes != null && routes.length() > 0) config.put("allowed_routes", routes)
                        if (newConfig.has("local_route_bypass") && !newConfig.isNull("local_route_bypass")) config.put("local_route_bypass", newConfig.getJSONArray("local_route_bypass"))
                    }
                    physicalAddresses = local
                    val next = Native.request("plan", JSONObject().put("config", config).put("local", local))
                    if (next.toString() != plan.toString()) {
                        val old = tun
                        try { tun = establish(next) } finally { old?.close() }
                        plan = next
                    }
                }
                val fd = tun?.fileDescriptor ?: break
                var traffic = false
                for (i in 0 until 64) {
                    try {
                        val count = Os.read(fd, buffer, 0, buffer.size)
                        if (count <= 0) break
                        traffic = true; send(handle, 0, buffer.copyOf(count), udp)
                    } catch (e: ErrnoException) {
                        if (e.errno == OsConstants.EAGAIN || e.errno == OsConstants.EINTR) break
                        throw e
                    }
                }
                for (i in 0 until 64) {
                    val datagram = DatagramPacket(buffer, buffer.size)
                    try { udp.receive(datagram); traffic = true; send(handle, 1, buffer.copyOf(datagram.length), udp) }
                    catch (_: SocketTimeoutException) { break }
                }
                val now = SystemClock.elapsedRealtime()
                if (now - lastTimer >= 100) { send(handle, 2, byteArrayOf(), udp); lastTimer = now }
                if (now - lastStatus >= 1000) {
                    val stats = Native.request("stats", JSONObject().put("id", handle))
                    if (!stats.isNull("handshake_seconds")) {
                        handshaken = true
                        if (stats.getLong("handshake_seconds") > 180) {
                            throw currentGateway.get()?.let { GatewayUnavailable(it, "WireGuard 握手已失效") }
                                ?: java.io.IOException("WireGuard 握手已失效")
                        }
                        dataPlaneReady.set(true)
                        details = "VPN IP：${plan.getString("address")}\n连接时长：${(now - started) / 1000} 秒\n上传：${stats.getLong("tx_bytes")} B\n下载：${stats.getLong("rx_bytes")} B\nDNS：${plan.optString("dns", "系统默认")}\n路由：${plan.getJSONArray("routes")}\n最近握手：${stats.getLong("handshake_seconds")} 秒前"
                        val (up, down) = rateMeter.sample(now, stats.getLong("tx_bytes"), stats.getLong("rx_bytes"))
                        report("已连接 ${plan.getString("address")} · ↑ ${TrafficFormat.rate(up)} ↓ ${TrafficFormat.rate(down)}")
                    } else if (now - started > 20000) {
                        throw currentGateway.get()?.let { GatewayUnavailable(it, "WireGuard 握手超时") }
                            ?: java.io.IOException("WireGuard 握手超时")
                    }
                    lastStatus = now
                }
                if (sampler != null && probeWorkers != null && now >= nextSample &&
                    sampling.compareAndSet(false, true)) {
                    nextSample = now + QUALITY_SAMPLE_INTERVAL_MS
                    val policy = qualityPolicy.get()
                    val context = if (dataPlaneReady.get() &&
                        policy != null && now - policy.refreshedAt <= QUALITY_MAX_POLICY_AGE_MS) {
                        currentGateway.get()?.let { policy.candidates.qualityContext(it) }
                    } else null
                    val addresses = physicalAddresses.toString()
                    sampler.execute {
                        try {
                            if (context == null) {
                                quality.reset()
                                qualityProposal.set(null)
                                qualityObservation.set(null)
                            } else {
                                probeWorkers.corePoolSize = minOf(QUALITY_MAX_PROBE_WORKERS, context.gateways.size)
                                val samples = cluster.sample(context.gateways, network, probeWorkers, 1, QUALITY_SAMPLE_BUDGET_MS)
                                if (qualityPolicy.get()?.candidates?.qualityContext(context.current) != context) {
                                    quality.reset()
                                    qualityProposal.set(null)
                                    qualityObservation.set(null)
                                } else {
                                    val reading = quality.record(context, addresses, now, samples) { !cluster.coolingDown(it) }
                                    qualityObservation.set(reading?.let { QualityObservation(context, it.stats, addresses, now) })
                                    val move = reading?.move
                                    if (move != null && now >= qualityCooldownUntil) {
                                        qualityProposal.set(QualityProposal(move, network, addresses, now))
                                    }
                                }
                            }
                        } catch (_: InterruptedException) {
                            Thread.currentThread().interrupt()
                        } catch (e: Exception) {
                            quality.reset()
                            qualityProposal.set(null)
                            qualityObservation.set(null)
                            if (!stopping.get()) Diagnostics.event("网关质量探测失败：${Diagnostics.logError(e)}")
                        } finally {
                            sampling.set(false)
                        }
                    }
                }
                val heartbeatDue = now - lastHeartbeat >= 30000
                val confirmationDue = dataPlaneReady.get() && currentGateway.get()?.revision?.let { it != confirmedRevision.get() } == true
                    && now - lastConfirmation >= 5000
                val qualityMoveDue = qualityProposal.get() != null
                val qualityReportDue = qualityObservation.get() != null && now - lastQualityReport >= QUALITY_REPORT_INTERVAL_MS
                if ((heartbeatDue || confirmationDue || qualityMoveDue || qualityReportDue) &&
                    heartbeatBusy.compareAndSet(false, true)) {
                    if (heartbeatDue) lastHeartbeat = now
                    if (confirmationDue) lastConfirmation = now
                    if (qualityReportDue) lastQualityReport = now
                    heartbeat.execute {
                        try {
                            var current = currentGateway.get()
                            if (heartbeatDue) {
                                update.set(client.heartbeat())
                                if (current != null) current = refreshGateway(current)
                            }
                            val proposal = if (qualityMoveDue) qualityProposal.getAndSet(null) else null
                            if (proposal != null && current != null && current == proposal.move.from &&
                                matchesPhysicalNetwork(proposal.addresses, proposal.sampledAt)) {
                                if (!heartbeatDue) current = refreshGateway(current)
                                val target = qualityPolicy.get()?.candidates?.preferredGateway(proposal.move)
                                if (current == proposal.move.from && target != null && !cluster.coolingDown(target.id) &&
                                    matchesPhysicalNetwork(proposal.addresses, proposal.sampledAt)) {
                                    Diagnostics.event("VPN 网关持续丢包，切换至丢包率更低的网关 ${target.id}")
                                    throw GatewayMove(proposal)
                                }
                            }
                            if (current != null && dataPlaneReady.get() && confirmedRevision.get() != current.revision) {
                                try {
                                    client.confirmGateway(current)
                                    confirmedRevision.set(current.revision)
                                } catch (_: GatewayPending) {
                                    refreshGateway(current)
                                }
                            } else if (current != null && dataPlaneReady.get() && qualityReportDue) {
                                val policy = qualityPolicy.get()
                                val observation = qualityObservation.get()
                                if (observation != null && policy != null &&
                                    SystemClock.elapsedRealtime() - policy.refreshedAt <= QUALITY_MAX_POLICY_AGE_MS &&
                                    policy.candidates.qualityContext(current) == observation.context &&
                                    matchesPhysicalNetwork(observation.addresses, observation.sampledAt)) {
                                    try {
                                        client.confirmGateway(observation.context.current, observation.stats)
                                    } catch (_: GatewayPending) {
                                        refreshGateway(current)
                                    }
                                }
                            }
                        } catch (e: Exception) {
                            clearQualityPolicy()
                            if (connection.gateway == null || e is GatewayMove || e is ApiError && e.stopsTunnel) failure.set(e)
                            else if (!stopping.get()) Diagnostics.event("网关管理请求失败：${Diagnostics.logError(e)}")
                        } finally { heartbeatBusy.set(false) }
                    }
                }
                if (!traffic) Thread.sleep(15)
            }
        } finally {
            client.cancel(); heartbeat.shutdownNow(); sampler?.shutdownNow(); probeWorkers?.shutdownNow(); closeIO()
            Native.request("destroy", JSONObject().put("id", handle))
        }
    }
    private fun establish(plan: JSONObject): ParcelFileDescriptor {
        check(!stopping.get())
        val builder = Builder().setSession("易链").setMtu(plan.getInt("mtu")).setBlocking(false)
            .addAddress(plan.getString("address"), 32).allowFamily(OsConstants.AF_INET6)
        val routes = plan.getJSONArray("routes")
        for (i in 0 until routes.length()) { val route = routes.getString(i).split('/'); builder.addRoute(route[0], route[1].toInt()) }
        if (!plan.isNull("dns")) builder.addDnsServer(plan.getString("dns"))
        return builder.establish() ?: throw java.io.IOException("VPN 授权已撤销")
    }
    private fun send(handle: Long, kind: Int, packet: ByteArray, udp: DatagramSocket) {
        val result = ByteBuffer.wrap(Native.process(handle, kind, packet))
        while (result.hasRemaining()) {
            val target = result.get().toInt(); val size = result.int
            require(size in 0..result.remaining()) { "无效原生报文" }
            val data = ByteArray(size); result.get(data)
            if (target == 0) udp.send(DatagramPacket(data, data.size))
            else {
                val fd = tun?.fileDescriptor ?: return
                try { Os.write(fd, data, 0, data.size) }
                catch (e: ErrnoException) { if (e.errno != OsConstants.EAGAIN) throw e }
            }
        }
    }
}
