package com.biubiu.vpn

import android.net.Network
import android.os.SystemClock
import org.json.JSONObject
import java.io.IOException
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.net.SocketTimeoutException
import java.security.SecureRandom
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.ExecutorCompletionService
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

internal data class GatewayChoice(val id: String, val revision: Long) {
    fun addTo(body: JSONObject): JSONObject = body.put("gateway_id", id).put("revision", revision)
}

internal data class Gateway(val id: String, val probeEndpoint: String)

internal data class GatewayCandidates(
    val defaultId: String,
    val targetId: String?,
    val activeId: String?,
    val revision: Long,
    val gateways: List<Gateway>,
) {
    companion object {
        fun parse(value: JSONObject): GatewayCandidates {
            val list = value.getJSONArray("gateways")
            return GatewayCandidates(
                value.getString("default_gateway_id"),
                value.takeUnless { it.isNull("target_gateway_id") }?.getString("target_gateway_id"),
                value.takeUnless { it.isNull("active_gateway_id") }?.getString("active_gateway_id"),
                value.getLong("revision"),
                (0 until minOf(list.length(), 64)).map { index ->
                    val node = list.getJSONObject(index)
                    Gateway(node.getString("id"), node.getString("probe_endpoint"))
                },
            )
        }
    }
}

internal class GatewayCluster(private val protect: (DatagramSocket) -> Boolean) {
    data class Connection(val config: JSONObject, val gateway: GatewayChoice?)
    data class Probe(val successes: Int, val meanNs: Long)
    private val failedUntil = ConcurrentHashMap<String, Long>()

    fun failed(gateway: GatewayChoice) {
        failedUntil[gateway.id] = SystemClock.elapsedRealtime() + 60000L
    }

    private fun coolingDown(id: String): Boolean =
        (failedUntil[id] ?: 0L) > SystemClock.elapsedRealtime()

    fun prepare(client: Api, network: Network): Connection {
        var candidates = client.candidates()
        val legacy = if (candidates == null) {
            val initial = waitSwitch { client.register() }
            candidates = client.candidates()
            initial
        } else null
        var list = candidates ?: return Connection(checkNotNull(legacy), null)
        val deadline = SystemClock.elapsedRealtime() + 22000L
        revision@ for (attempt in 0 until 3) {
            var error: IOException = IOException("当前没有可用的已授权 VPN 网关")
            for (gateway in rank(list, network)) {
                val choice = GatewayChoice(gateway.id, list.revision)
                while (true) {
                    try { return Connection(client.selectGateway(choice), choice) }
                    catch (e: GatewayPending) {
                        if (SystemClock.elapsedRealtime() >= deadline) throw e
                        val latest = client.candidates() ?: throw e
                        if (latest.revision != choice.revision) {
                            list = latest
                            continue@revision
                        }
                        if (latest.gateways.none { it.id == choice.id }) {
                            error = e
                            break
                        }
                        Thread.sleep(750L)
                    } catch (e: ApiError) {
                        if (e.code != 2002 || SystemClock.elapsedRealtime() >= deadline) throw e
                        val latest = client.candidates() ?: throw e
                        if (latest.revision == choice.revision && latest.defaultId == list.defaultId && latest.gateways.any { it.id == choice.id }) throw e
                        list = latest
                        continue@revision
                    }
                }
            }
            throw error
        }
        throw GatewayPending()
    }

    fun instructedSwitch(candidates: GatewayCandidates, current: GatewayChoice): Boolean {
        if (candidates.activeId != current.id && candidates.gateways.none { it.id == current.id }) return true
        val target = candidates.gateways.firstOrNull { it.id == candidates.targetId } ?: return false
        return target.id != current.id && !coolingDown(target.id)
    }

    private fun rank(candidates: GatewayCandidates, network: Network): List<Gateway> {
        if (candidates.gateways.size <= 1) return candidates.gateways
        val executor = Executors.newFixedThreadPool(minOf(8, candidates.gateways.size))
        val completed = ExecutorCompletionService<Pair<Gateway, Probe>>(executor)
        val samples = mutableMapOf<String, Probe>()
        val end = SystemClock.elapsedRealtime() + 2000L
        try {
            candidates.gateways.forEach { gateway -> completed.submit { gateway to probe(gateway, network) } }
            repeat(candidates.gateways.size) {
                val next = completed.poll((end - SystemClock.elapsedRealtime()).coerceAtLeast(0L), TimeUnit.MILLISECONDS) ?: return@repeat
                val (gateway, sample) = next.get()
                samples[gateway.id] = sample
            }
        } finally {
            executor.shutdownNow()
        }
        val missing = Probe(0, 0)
        return candidates.gateways.sortedWith(compareBy<Gateway>(
            { coolingDown(it.id) },
            { it.id != candidates.targetId },
            { -(samples[it.id] ?: missing).successes },
            { (samples[it.id] ?: missing).meanNs },
            { it.id != candidates.defaultId },
            { it.id },
        ))
    }

    private fun probe(gateway: Gateway, network: Network): Probe {
        var successes = 0
        var totalNs = 0L
        try {
            val split = gateway.probeEndpoint.lastIndexOf(':')
            require(split > 0)
            val host = gateway.probeEndpoint.substring(0, split).removePrefix("[").removeSuffix("]")
            val port = gateway.probeEndpoint.substring(split + 1).toInt()
            val address = network.getAllByName(host).firstOrNull { it is java.net.Inet4Address } ?: return Probe(0, 0)
            DatagramSocket(null).use { socket ->
                if (!protect(socket)) return Probe(0, 0)
                network.bindSocket(socket)
                socket.bind(InetSocketAddress(0))
                socket.connect(address, port)
                repeat(3) {
                    if (Thread.currentThread().isInterrupted) return@repeat
                    val request = ByteArray(24).also {
                        random.nextBytes(it)
                        "YILIAN01".toByteArray(Charsets.US_ASCII).copyInto(it)
                    }
                    val sentAt = System.nanoTime()
                    socket.send(DatagramPacket(request, request.size))
                    val deadline = SystemClock.elapsedRealtime() + 350L
                    val data = ByteArray(25)
                    while (true) {
                        val remaining = deadline - SystemClock.elapsedRealtime()
                        if (remaining <= 0) break
                        socket.soTimeout = remaining.toInt()
                        val packet = DatagramPacket(data, data.size)
                        try { socket.receive(packet) } catch (_: SocketTimeoutException) { break }
                        if (packet.length == request.size && data.copyOf(packet.length).contentEquals(request)) {
                            successes++
                            totalNs += System.nanoTime() - sentAt
                            break
                        }
                    }
                }
            }
        } catch (_: IOException) {
        } catch (_: IllegalArgumentException) {
        }
        return Probe(successes, if (successes == 0) 0L else totalNs / successes)
    }

    private fun <T> waitSwitch(request: () -> T): T {
        val deadline = SystemClock.elapsedRealtime() + 22000L
        while (true) {
            try { return request() }
            catch (e: GatewayPending) {
                if (SystemClock.elapsedRealtime() >= deadline) throw e
                Thread.sleep(750L)
            }
        }
    }

    companion object { private val random = SecureRandom() }
}
