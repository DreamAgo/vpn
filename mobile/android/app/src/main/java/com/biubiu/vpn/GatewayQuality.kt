package com.biubiu.vpn

import org.json.JSONObject

internal data class GatewayQualityContext(
    val current: GatewayChoice,
    val defaultId: String,
    val targetId: String?,
    val gateways: List<Gateway>,
)

internal data class GatewayQualityMove(
    val context: GatewayQualityContext,
    val target: Gateway,
    val probeAddress: String? = null,
) {
    val from: GatewayChoice get() = context.current
}

internal data class GatewayQualityStats(val rttMs: Long?, val lossPct: Int) {
    fun toJson(): JSONObject = JSONObject().put("rtt_ms", rttMs ?: JSONObject.NULL).put("loss_pct", lossPct)
}

internal data class GatewayQualityReading(val stats: GatewayQualityStats, val move: GatewayQualityMove?)

/** Records one result per gateway per round on the background sampler. */
internal class GatewayQuality {
    private val rounds = ArrayDeque<Map<String, GatewayCluster.Probe>>()
    private var context: GatewayQualityContext? = null
    private var localNetwork: String? = null
    private var lastSampleAt = 0L
    private var sinceAssessment = 0
    private var previousBest: Gateway? = null
    private var bestStreak = 0

    fun reset() {
        rounds.clear()
        context = null
        localNetwork = null
        lastSampleAt = 0L
        sinceAssessment = 0
        previousBest = null
        bestStreak = 0
    }

    fun record(
        current: GatewayQualityContext,
        network: String,
        now: Long,
        samples: Map<String, GatewayCluster.Probe>,
        eligible: (String) -> Boolean,
    ): GatewayQualityReading? {
        if (context != current || localNetwork != network || now - lastSampleAt > MAX_SAMPLE_GAP_MS) reset()
        context = current
        localNetwork = network
        lastSampleAt = now
        if (rounds.size == WINDOW) rounds.removeFirst()
        rounds.addLast(samples)
        sinceAssessment++
        if (rounds.size < WINDOW) return null
        val active = score(current.current.id)
        val move = if (sinceAssessment >= ASSESS_EVERY) {
            sinceAssessment = 0
            assess(current, active, eligible)
        } else null
        return active?.let {
            val rttMs = if (it.successes > 0) it.meanNs / 1000000L else null
            GatewayQualityReading(GatewayQualityStats(rttMs, 100 * (WINDOW - it.successes) / WINDOW), move)
        }
    }

    private fun assess(
        current: GatewayQualityContext,
        active: GatewayCluster.Probe?,
        eligible: (String) -> Boolean,
    ): GatewayQualityMove? {
        val best = if (active == null || current.targetId != null) null else current.gateways.asSequence()
            .filter { it.id != current.current.id && eligible(it.id) }
            .mapNotNull { gateway -> score(gateway.id)?.let { gateway to it } }
            .filter { (_, score) ->
                score.successes > 0 && score.successes - active.successes >= MIN_SUCCESS_IMPROVEMENT
            }
            .minWithOrNull(compareBy(
                { -it.second.successes },
                { it.first.id != current.defaultId },
                { it.first.id },
            ))?.first
        bestStreak = when {
            best == null -> 0
            best == previousBest -> bestStreak + 1
            else -> 1
        }
        previousBest = best
        if (best == null || bestStreak < REQUIRED_ASSESSMENTS) return null
        // Overlapping windows can repeat one past loss burst in every assessment.
        val activeRecent = score(current.current.id, RECENT_WINDOW) ?: return null
        val bestRecent = score(best.id, RECENT_WINDOW) ?: return null
        return if (bestRecent.successes > activeRecent.successes) GatewayQualityMove(current, best, bestRecent.address) else null
    }

    private fun score(id: String, sampleCount: Int = WINDOW): GatewayCluster.Probe? {
        var successes = 0
        var totalNs = 0L
        val start = rounds.size - sampleCount
        val address = rounds[start][id]?.address
        for (index in start until rounds.size) {
            val sample = rounds[index][id] ?: return null
            if (sample.address != address) return null
            successes += sample.successes
            totalNs += sample.meanNs
        }
        return GatewayCluster.Probe(successes, if (successes == 0) 0 else totalNs / successes, address)
    }

    private companion object {
        const val WINDOW = 20
        const val ASSESS_EVERY = 5
        const val REQUIRED_ASSESSMENTS = 3
        const val RECENT_WINDOW = ASSESS_EVERY * (REQUIRED_ASSESSMENTS - 1)
        const val MIN_SUCCESS_IMPROVEMENT = 2
        const val MAX_SAMPLE_GAP_MS = 5000L
    }
}
