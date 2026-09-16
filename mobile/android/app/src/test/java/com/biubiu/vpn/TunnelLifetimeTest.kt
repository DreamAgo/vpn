package com.biubiu.vpn

import org.junit.Assert.*
import org.junit.Test

class TunnelLifetimeTest {
    @Test fun stopCannotRestartUntilIoHasBeenReleased() {
        val lifetime = TunnelLifetime(); val first = lifetime.begin()!!
        assertTrue(lifetime.accepts(first))
        lifetime.stop(first)
        assertFalse(lifetime.accepts(first)); assertNull(lifetime.begin()); assertTrue(lifetime.running())
        assertTrue(lifetime.finish(first)); assertFalse(lifetime.running())
        val second = lifetime.begin()!!
        assertFalse(lifetime.finish(first)); assertTrue(lifetime.accepts(second))
        lifetime.stop(first); assertTrue(lifetime.accepts(second))
    }
    @Test fun simultaneousStartHasSingleOwner() {
        val lifetime = TunnelLifetime()
        val owners = java.util.Collections.synchronizedList(mutableListOf<Long>())
        val threads = (1..16).map { Thread { lifetime.begin()?.let(owners::add) }.apply { start() } }
        threads.forEach { it.join() }
        assertEquals(1, owners.size)
    }
}
