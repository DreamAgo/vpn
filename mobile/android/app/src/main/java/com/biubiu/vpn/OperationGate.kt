package com.biubiu.vpn

/** Cancel invalidates every queued UI callback even when the worker ignores interruption. */
class OperationGate {
    @Volatile private var generation = 0
    @Synchronized fun begin(): Int { generation++; return generation }
    @Synchronized fun cancel() { generation++ }
    fun accepts(ticket: Int) = ticket == generation
    fun current() = generation
}
