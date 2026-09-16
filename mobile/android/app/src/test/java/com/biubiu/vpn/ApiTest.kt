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
    @Test fun cancelledCompletedFeishuLoginRevokesNewSessionWithoutPersisting() {
        val store = MemoryStore(); val entered = CountDownLatch(1); val release = CountDownLatch(1)
        var revoked = false
        val api = Api(store) { path, body, _ ->
            if (path == "/auth/feishu/poll") {
                entered.countDown(); release.await()
                ok(JSONObject().put("status", "complete").put("username", "real-user").put("login", JSONObject().put("access_token", "new-access").put("refresh_token", "new-refresh")))
            } else {
                assertEquals("/auth/logout", path); assertEquals("new-refresh", body.getString("refresh_token")); revoked = true; ok()
            }
        }
        val worker = Thread { try { api.publicRequest("https://example.com", "/auth/feishu/poll") } catch (_: Exception) {} }.apply { start() }
        assertTrue(entered.await(5, java.util.concurrent.TimeUnit.SECONDS)); api.cancel(); release.countDown(); worker.join(5000)
        assertFalse(worker.isAlive); assertTrue(revoked); assertEquals(0, store.writes); assertEquals("refresh", store.value.getString("refresh"))
    }
    @Test fun failedFeishuPersistenceRevokesIssuedSessionAndKeepsPrevious() {
        val previous = JSONObject().put("server", "https://example.com").put("username", "real-user").put("private", "private").put("public", "public").put("refresh", "old-refresh")
        var revoked = false
        val api = Api(object : CredentialStore {
            override fun read() = previous
            override fun write(value: JSONObject) { throw java.io.IOException("storage failure") }
            override fun clear() { fail("must not clear existing account") }
        }) { path, body, _ -> assertEquals("/auth/logout", path); assertEquals("new-refresh", body.getString("refresh_token")); revoked = true; ok() }
        try { api.acceptFeishu("https://example.com", JSONObject().put("username", "real-user").put("login", JSONObject().put("access_token", "new-access").put("refresh_token", "new-refresh"))); fail() } catch (_: java.io.IOException) {}
        assertTrue(revoked); assertEquals("old-refresh", api.saved().getString("refresh"))
    }
    @Test fun feishuUsesServerUsernameAndReusesOnlyMatchingKeys() {
        val store = MemoryStore(); store.value.put("username", "real-user").put("private", "old-private")
        val api = Api(store)
        api.acceptFeishu("https://example.com", JSONObject().put("username", "real-user").put("login", JSONObject().put("access_token", "new-access").put("refresh_token", "new-refresh")))
        assertEquals("real-user", store.value.getString("username")); assertEquals("new-refresh", store.value.getString("refresh")); assertEquals("old-private", store.value.getString("private"))
    }
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
