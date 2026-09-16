package com.biubiu.vpn

import org.junit.Assert.*
import org.junit.Test

class NetworkSelectionTest {
    private val wifi = NetworkSelection.Candidate(20L, 13)
    private val cell = NetworkSelection.Candidate(10L, 12)
    @Test fun activeVpnCannotCauseEnumerationOrderReconnectLoop() {
        assertEquals(20L, NetworkSelection.select(listOf(cell, wifi), 99L, 20L))
        assertEquals(20L, NetworkSelection.select(listOf(wifi, cell), 99L, 20L))
    }
    @Test fun lossAndWifiUpgradeSwitchUnderlyingNetwork() {
        assertEquals(10L, NetworkSelection.select(listOf(cell), 99L, 20L))
        assertEquals(20L, NetworkSelection.select(listOf(cell, wifi), 99L, 10L))
        assertNull(NetworkSelection.select(emptyList(), 99L, 20L))
    }
    @Test fun equivalentNetworksRetainCurrentAndPhysicalDefaultWins() {
        val other = NetworkSelection.Candidate(30L, 13)
        assertEquals(30L, NetworkSelection.select(listOf(wifi, other), 99L, 30L))
        assertEquals(20L, NetworkSelection.select(listOf(cell, wifi), 10L, 20L))
    }
    @Test fun selectionIsConsistentBeforeAndAfterVpnBecomesDefault() {
        val initial = NetworkSelection.select(listOf(cell, wifi), 10L, null)
        assertEquals(20L, initial)
        assertEquals(initial, NetworkSelection.select(listOf(wifi, cell), 99L, initial))
    }

}
