package com.biubiu.vpn

/** A stopped generation remains owned until its worker has released all IO. */
class TunnelLifetime {
    private var sequence = 0L
    private var owner: Long? = null
    private var stopping = false
    @Synchronized fun begin(): Long? {
        if (owner != null) return null
        sequence++; owner = sequence; stopping = false
        return sequence
    }
    @Synchronized fun stop(token: Long) { if (owner == token) stopping = true }
    @Synchronized fun accepts(token: Long): Boolean = owner == token && !stopping
    @Synchronized fun finish(token: Long): Boolean {
        if (owner != token) return false
        owner = null; stopping = false
        return true
    }
    @Synchronized fun running(): Boolean = owner != null
}
