package com.biubiu.vpn

import org.json.JSONObject

object Native {
    init { System.loadLibrary("vpn_mobile") }
    @JvmStatic external fun call(operation: String, input: String): String
    @JvmStatic external fun process(handle: Long, kind: Int, input: ByteArray): ByteArray
    fun request(operation: String, input: JSONObject = JSONObject()): JSONObject {
        val result = JSONObject(call(operation, input.toString()))
        if (result.has("error")) throw IllegalArgumentException(result.getString("error"))
        return result.getJSONObject("data")
    }
}
