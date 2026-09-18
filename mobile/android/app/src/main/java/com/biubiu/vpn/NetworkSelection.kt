package com.biubiu.vpn

object NetworkSelection {
    data class Candidate(val id: Long, val priority: Int)
    fun select(candidates: List<Candidate>, activePhysical: Long?, current: Long?): Long? {
        val best = candidates.maxOfOrNull { it.priority } ?: return null
        if (candidates.any { it.id == activePhysical && it.priority == best }) return activePhysical
        if (candidates.any { it.id == current && it.priority == best }) return current
        return candidates.filter { it.priority == best }.minByOrNull { it.id }?.id
    }
}
