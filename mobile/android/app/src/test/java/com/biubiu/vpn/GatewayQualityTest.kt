package com.biubiu.vpn

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class GatewayQualityTest {
    private val local = Gateway("local", "local.example:51822")
    private val east = Gateway("east", "east.example:51822")
    private val west = Gateway("west", "west.example:51822")
    private val current = GatewayChoice(local.id, 3)
    private val candidates = GatewayCandidates(local.id, null, local.id, current.revision, listOf(local, east, west))
    private val context = checkNotNull(candidates.qualityContext(current))

    private val lost = GatewayCluster.Probe(0, 0)
    private val quick = GatewayCluster.Probe(1, 100)
    private val slow = GatewayCluster.Probe(1, 1000)

    @Test fun requiresThreeAssessmentsAndChoosesLowerLossBeforeLowerLatency() {
        val quality = GatewayQuality()
        for (round in 1..30) {
            val samples = mapOf(
                local.id to if (round % 2 == 0) lost else quick,
                east.id to slow,
                west.id to if (round % 4 == 0) lost else quick,
            )
            val move = quality.record(context, "wifi", round * 2000L, samples) { true }?.move
            assertEquals(if (round == 30) east else null, move?.target)
        }
    }

    @Test fun transientLossIncompleteSamplesAndFailedCandidatesKeepTheCurrentGateway() {
        val quality = GatewayQuality()
        for (round in 1..40) {
            val samples = buildMap {
                put(local.id, if (round % 20 == 0) lost else slow)
                put(east.id, quick)
            }
            assertNull(quality.record(context, "wifi", round * 2000L, samples) { true }?.move)
        }
        for (round in 41..80) {
            val samples = mapOf(local.id to lost, east.id to quick)
            assertNull(quality.record(context, "wifi", round * 2000L, samples) { it != east.id }?.move)
        }
    }

    @Test fun aNetworkChangeDiscardsPreviousMeasurements() {
        val quality = GatewayQuality()
        val samples = mapOf(local.id to lost, east.id to quick, west.id to lost)
        for (round in 1..29) {
            assertNull(quality.record(context, "wifi", round * 2000L, samples) { true }?.move)
        }
        for (round in 30..59) {
            val move = quality.record(context, "mobile", round * 2000L, samples) { true }?.move
            assertEquals(if (round == 59) east else null, move?.target)
        }
    }

    @Test fun anOldTwoPacketBurstDoesNotTriggerSwitchingThroughOverlappingWindows() {
        val quality = GatewayQuality()
        for (round in 1..40) {
            val samples = mapOf(local.id to if (round in 11..12) lost else quick, east.id to slow)
            assertNull(quality.record(context, "wifi", round * 2000L, samples) { true }?.move)
        }
    }

    @Test fun switchingRequiresTheSameAuthorizedAutomaticPolicyAndProbeEndpoints() {
        val move = GatewayQualityMove(context, east)
        assertEquals(east, candidates.copy(gateways = candidates.gateways.reversed()).preferredGateway(move))
        assertNull(candidates.copy(targetId = west.id).preferredGateway(move))
        assertNull(candidates.copy(revision = 4).preferredGateway(move))
        assertNull(candidates.copy(gateways = listOf(local, west)).preferredGateway(move))
        assertNull(candidates.copy(gateways = listOf(local, east.copy(probeEndpoint = "new.example:51822"), west)).preferredGateway(move))
    }

    @Test fun reconnectKeepsTheAuthorizedActiveGatewayWhenTheQualityMoveCannotBeUsed() {
        val move = GatewayQualityMove(context, east)
        val eastReply = mapOf(east.id to GatewayCluster.Probe(1, 1))
        val localOnly = mapOf(local.id to GatewayCluster.Probe(3, 1), east.id to GatewayCluster.Probe(0, 0))
        assertEquals(east, candidates.preferredForReconnect(move, eastReply))
        assertEquals(local, candidates.preferredForReconnect(move, localOnly))
        assertEquals(local, candidates.copy(gateways = listOf(local, west)).preferredForReconnect(move, eastReply))
        assertEquals(local, candidates.copy(revision = 4).preferredForReconnect(move, eastReply))
        assertNull(candidates.copy(targetId = west.id).preferredForReconnect(move, localOnly))
        assertNull(candidates.copy(activeId = west.id).preferredForReconnect(move, localOnly))
        assertNull(candidates.copy(gateways = listOf(east, west)).preferredForReconnect(move, localOnly))
    }

    @Test fun reportsOnlyCompleteCurrentGatewayWindowsWhilePinnedOrDefaultOnly() {
        val quality = GatewayQuality()
        val pinned = checkNotNull(candidates.copy(targetId = local.id).qualityContext(current))
        val defaultOnly = checkNotNull(candidates.copy(gateways = listOf(local)).qualityContext(current))
        assertEquals(listOf(local), pinned.gateways)
        assertEquals(listOf(local), defaultOnly.gateways)

        for (round in 1..19) {
            val currentProbe = if (round % 2 == 0) GatewayCluster.Probe(1, 30000000L) else lost
            assertNull(quality.record(pinned, "wifi", round * 2000L, mapOf(local.id to currentProbe)) { true })
        }
        val reading = quality.record(pinned, "wifi", 40000L, mapOf(local.id to GatewayCluster.Probe(1, 10000000L))) { true }
        assertEquals(GatewayQualityStats(28L, 50), reading?.stats)
        assertNull(reading?.move)

        for (round in 21..39) {
            assertNull(quality.record(defaultOnly, "wifi", round * 2000L, mapOf(local.id to lost)) { true })
        }
        val allLost = quality.record(defaultOnly, "wifi", 80000L, mapOf(local.id to lost)) { true }
        assertEquals(GatewayQualityStats(null, 100), allLost?.stats)
        assertNull(quality.record(defaultOnly, "wifi", 82000L, emptyMap()) { true })
    }
}
