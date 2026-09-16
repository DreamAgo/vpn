package com.biubiu.vpn

import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities

object PhysicalNetwork {
    fun choose(cm: ConnectivityManager, current: Network? = null): Network? {
        val networks = cm.allNetworks.toList()
        val candidates = networks.mapNotNull { network ->
            val caps = cm.getNetworkCapabilities(network) ?: return@mapNotNull null
            if (!caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) || !caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)) return@mapNotNull null
            val transport = when {
                caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> 4
                caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> 3
                caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> 2
                else -> 1
            }
            val validated = if (caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)) 10 else 0
            NetworkSelection.Candidate(network.networkHandle, validated + transport)
        }
        val selected = NetworkSelection.select(candidates, cm.activeNetwork?.networkHandle, current?.networkHandle)
        return networks.firstOrNull { it.networkHandle == selected }
    }
}
