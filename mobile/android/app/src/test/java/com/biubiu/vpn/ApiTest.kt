package com.biubiu.vpn

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.util.concurrent.CountDownLatch

class ApiTest {
    private class MemoryStore : CredentialStore {
        var value = JSONObject().put("server", "https://example.com").put("access", "old").put("refresh", "refresh").put("public", "public")
        var writes = 0
        override fun read() = JSONObject(value.toString())
        override fun write(value: JSONObject) { this.value = JSONObject(value.toString()); writes++ }
        override fun clear() { value = JSONObject() }
    }
    private fun ok(data: JSONObject? = JSONObject()) = JSONObject().put("code", 0).put("data", data ?: JSONObject.NULL)
    private fun expired() = JSONObject().put("code", 1002)
    @Test fun expiredAccessRefreshesOnceAndRetriesWithNewToken() {
        val store = MemoryStore(); val calls = mutableListOf<String>()
        val api = Api(store) { path, _, token ->
            calls.add(path)
            if (path == "/auth/refresh") ok(JSONObject().put("access_token", "new"))
            else if (token == "old") expired() else ok(null)
        }
        assertEquals(0, api.heartbeat().length())
        assertEquals(listOf("/peers/heartbeat", "/auth/refresh", "/peers/heartbeat"), calls)
        assertEquals("new", store.value.getString("access"))
    }
    @Test fun failedRefreshClearsSessionAndDoesNotLoop() {
        val store = MemoryStore(); var count = 0
        val api = Api(store) { _, _, _ -> count++; expired() }
        try { api.heartbeat(); fail() } catch (e: ApiError) { assertTrue(e.fatal) }
        assertEquals(2, count); assertEquals(0, store.value.length())
    }
    @Test fun revokedSessionClearsCredentialsWithoutRefresh() {
        val store = MemoryStore(); var count = 0
        val api = Api(store) { _, _, _ -> count++; JSONObject().put("code", 1007) }
        try { api.heartbeat(); fail() } catch (_: ApiError) {}
        assertEquals(1, count); assertEquals(0, store.value.length())
    }
    @Test fun passwordChangeClearsRevokedTokens() {
        val store = MemoryStore()
        val api = Api(store) { path, body, _ -> assertEquals("/auth/change-password", path); assertEquals("new password", body.getString("new_password")); ok(null) }
        api.changePassword("old password", "new password")
        assertEquals(0, store.value.length()); assertEquals(0, api.saved().length())
    }
    @Test fun cancellationDuringRefreshCannotPersistTokens() {
        val store = MemoryStore(); val entered = CountDownLatch(1); val release = CountDownLatch(1)
        val api = Api(store) { path, _, _ ->
            if (path == "/auth/refresh") { entered.countDown(); release.await(); ok(JSONObject().put("access_token", "new")) } else expired()
        }
        var failure: Exception? = null
        val worker = Thread { try { api.heartbeat() } catch (e: Exception) { failure = e } }.apply { start() }
        assertTrue(entered.await(5, java.util.concurrent.TimeUnit.SECONDS))
        api.cancel(); release.countDown(); worker.join(5000)
        assertFalse(worker.isAlive); assertTrue(failure is IllegalStateException); assertEquals(0, store.writes)
    }
    @Test fun networkFailureDoesNotEraseRefreshToken() {
        val store = MemoryStore(); val api = Api(store) { _, _, _ -> throw java.io.IOException("offline") }
        try { api.heartbeat(); fail() } catch (_: java.io.IOException) {}
        assertEquals("refresh", store.value.getString("refresh"))
    }
    @Test fun logoutUsesRotatedRefreshTokenAfterRetry() {
        val store = MemoryStore(); var logoutCalls = 0
        val api = Api(store) { path, body, token ->
            if (path == "/auth/refresh") ok(JSONObject().put("access_token", "new").put("refresh_token", "rotated"))
            else {
                logoutCalls++
                if (token == "old") expired() else { assertEquals("rotated", body.getString("refresh_token")); ok(null) }
            }
        }
        api.logout(); assertEquals(2, logoutCalls); assertEquals(0, store.value.length())
    }

    @Test fun cancelledLogoutFinallyCannotClearNewCredentials() {
        val store = MemoryStore()
        val api = Api(store) { _, _, _ -> ok(null) }
        api.cancel()
        store.value.put("access", "new-session")
        try { api.logout(); fail() } catch (_: IllegalStateException) {}
        assertEquals("new-session", store.value.getString("access"))
    }
    @Test fun lateFatalResponseAfterCancellationCannotClearStore() {
        val store = MemoryStore(); val entered = CountDownLatch(1); val release = CountDownLatch(1)
        val api = Api(store) { _, _, _ -> entered.countDown(); release.await(); JSONObject().put("code", 1007) }
        val worker = Thread { try { api.heartbeat() } catch (_: Exception) {} }.apply { start() }
        assertTrue(entered.await(5, java.util.concurrent.TimeUnit.SECONDS))
        api.cancel(); store.value.put("access", "new-session"); release.countDown(); worker.join(5000)
        assertFalse(worker.isAlive); assertEquals("new-session", store.value.getString("access"))
    }

}
