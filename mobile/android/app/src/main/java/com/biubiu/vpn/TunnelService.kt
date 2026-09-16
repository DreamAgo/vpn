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
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

class TunnelService : VpnService() {
    companion object {
        @Volatile var status = "未连接"
        private val lifetime = TunnelLifetime()
        val running: Boolean get() = lifetime.running()
        const val STOP = "com.biubiu.vpn.STOP"
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
        generation = lifetime.begin() ?: return START_NOT_STICKY
        stopping.set(false)
        getSystemService(NotificationManager::class.java).createNotificationChannel(NotificationChannel("vpn", "VPN 连接", NotificationManager.IMPORTANCE_LOW))
        startForeground(1, notification("正在连接"))
        worker = Thread({ runConnection() }, "vpn-control").also { it.start() }
        return START_NOT_STICKY
    }
    private fun notification(text: String): Notification {
        val stop = PendingIntent.getService(this, 1, Intent(this, TunnelService::class.java).setAction(STOP), PendingIntent.FLAG_IMMUTABLE)
        val open = PendingIntent.getActivity(this, 2, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        return Notification.Builder(this, "vpn").setSmallIcon(android.R.drawable.stat_sys_upload_done).setContentTitle("易链")
            .setContentText(text).setContentIntent(open).setOngoing(true).addAction(Notification.Action.Builder(null, "断开", stop).build()).build()
    }
    private fun report(text: String) {
        if (stopping.get() || !lifetime.accepts(generation)) return
        status = text
        getSystemService(NotificationManager::class.java).notify(1, notification(text))
    }
    private fun closeIO() {
        socket?.close(); socket = null
        try { tun?.close() } catch (_: Exception) {} finally { tun = null }
    }
    private fun shutdown() {
        stopping.set(true); lifetime.stop(generation); api?.cancel(); closeIO(); worker?.interrupt()
        status = "正在断开"
        if (worker == null) { status = "已断开"; lifetime.finish(generation); stopForeground(STOP_FOREGROUND_REMOVE); stopSelf() }
    }
    override fun onRevoke() { shutdown(); super.onRevoke() }
    override fun onDestroy() { stopping.set(true); lifetime.stop(generation); api?.cancel(); closeIO(); worker?.interrupt(); if (worker == null) lifetime.finish(generation); super.onDestroy() }
    private fun runConnection() {
        var delay = 1000L
        try {
            while (!stopping.get()) {
                try {
                    val physical = PhysicalNetwork.choose(cm) ?: throw java.io.IOException("等待网络")
                    val client = Api(Vault(this)).also { it.network = physical; api = it }
                    report("正在注册设备")
                    val config = client.register()
                    check(!stopping.get())
                    handshaken = false
                    runTunnel(client, physical, config)
                    delay = 1000L
                } catch (e: Exception) {
                    if (stopping.get()) break
                    if (e is ApiError && (e.fatal || e.code == 1006) || e is IllegalArgumentException || e is IllegalStateException || e is org.json.JSONException) {
                        report("连接停止：${e.message}"); break
                    }
                    if (handshaken) { delay = 1000L; handshaken = false }
                    report("连接中断，${delay / 1000} 秒后重试：${e.message}")
                    Thread.sleep(delay); delay = (delay * 2).coerceAtMost(30000)
                } finally { api?.cancel(); api = null; closeIO() }
            }
        } catch (_: InterruptedException) {
        } finally {
            closeIO(); worker = null
            Handler(Looper.getMainLooper()).post {
                if (lifetime.finish(generation)) { if (stopping.get()) status = "已断开"; stopForeground(STOP_FOREGROUND_REMOVE); stopSelf() }
            }
        }
    }
    private fun localAddresses(network: Network): JSONArray = JSONArray().apply {
        cm.getLinkProperties(network)?.linkAddresses?.forEach { if (it.address is Inet4Address) put(it.address.hostAddress) }
    }
    private fun runTunnel(client: Api, network: Network, initial: JSONObject) {
        var config = initial
        var physicalAddresses = localAddresses(network)
        var plan = Native.request("plan", JSONObject().put("config", config).put("local", physicalAddresses))
        val endpoint = plan.getString("endpoint")
        val split = endpoint.lastIndexOf(':')
        val host = endpoint.substring(0, split)
        val address = network.getAllByName(host).firstOrNull { it is Inet4Address } ?: throw IllegalArgumentException("首版需要 IPv4 VPN 服务端地址")
        val udp = DatagramSocket(null)
        socket = udp
        if (!protect(udp)) throw java.io.IOException("无法保护 VPN 外层 socket")
        network.bindSocket(udp)
        udp.bind(InetSocketAddress(0)); udp.connect(address, endpoint.substring(split + 1).toInt()); udp.soTimeout = 1
        setUnderlyingNetworks(arrayOf(network))
        val handle = Native.request("create", JSONObject().put("config", config).put("private", client.saved().getString("private"))).getLong("id")
        val heartbeat = Executors.newSingleThreadExecutor()
        val heartbeatBusy = AtomicBoolean(false)
        val update = AtomicReference<JSONObject?>(null)
        val failure = AtomicReference<Exception?>(null)
        try {
            tun = establish(plan)
            check(!stopping.get())
            send(handle, 3, byteArrayOf(), udp)
            val buffer = ByteArray(65535)
            var lastNetworkCheck = 0L
            var lastTimer = 0L; var lastStatus = 0L; var lastHeartbeat = SystemClock.elapsedRealtime()
            val started = SystemClock.elapsedRealtime()
            report("正在进行 WireGuard 握手")
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
                        if (stats.getLong("handshake_seconds") > 180) throw java.io.IOException("WireGuard 握手已失效")
                        report("已连接 ${plan.getString("address")} · ↑ ${stats.getLong("tx_bytes")} B ↓ ${stats.getLong("rx_bytes")} B")
                    } else if (now - started > 20000) throw java.io.IOException("WireGuard 握手超时")
                    lastStatus = now
                }
                if (now - lastHeartbeat >= 30000 && heartbeatBusy.compareAndSet(false, true)) {
                    lastHeartbeat = now
                    heartbeat.execute {
                        try { update.set(client.heartbeat()) } catch (e: Exception) { failure.set(e) }
                        finally { heartbeatBusy.set(false) }
                    }
                }
                if (!traffic) Thread.sleep(15)
            }
        } finally {
            client.cancel(); heartbeat.shutdownNow(); closeIO()
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
