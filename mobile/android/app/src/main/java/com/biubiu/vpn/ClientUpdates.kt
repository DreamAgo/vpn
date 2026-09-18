package com.biubiu.vpn

import android.content.Context
import android.content.pm.PackageManager
import android.net.Network
import android.os.Build
import org.json.JSONObject
import java.io.File
import java.net.URI
import java.net.URL
import java.security.MessageDigest
import javax.net.ssl.HttpsURLConnection

data class UpdatePackage(val server: String, val version: String, val name: String, val url: String, val size: Long, val digest: String, val notes: String)
data class PackageIdentity(val name: String, val version: String?, val code: Long, val signers: Set<String>)

class ClientUpdates(private val context: Context, private val network: Network? = null) {
    @Volatile private var cancelled = false
    @Volatile private var connection: HttpsURLConnection? = null
    fun cancel() { cancelled = true; connection?.disconnect() }
    private fun checkActive() { check(!cancelled && !Thread.currentThread().isInterrupted) { "操作已取消" } }
    private fun connection(url: String): HttpsURLConnection {
        checkActive()
        return ((network ?: throw LocalFailure("没有可用网络")).openConnection(URL(url)) as HttpsURLConnection).also {
            connection = it; it.instanceFollowRedirects = false; it.connectTimeout = 10000; it.readTimeout = 10000
        }
    }
    fun check(server: String): UpdatePackage? {
        val origin = Api.validServer(server)
        val conn = connection("$origin/updates/latest.json")
        try {
            if (conn.responseCode != 200) throw LocalFailure("服务器尚未提供可用更新清单")
            val output = java.io.ByteArrayOutputStream()
            conn.inputStream.use { input ->
                val buffer = ByteArray(8192)
                while (true) { checkActive(); val n = input.read(buffer); if (n < 0) break; require(output.size() + n <= 1048576); output.write(buffer, 0, n) }
            }
            return parse(origin, JSONObject(output.toString("UTF-8")), BuildConfig.VERSION_NAME)
        } finally { conn.disconnect(); connection = null }
    }
    fun download(update: UpdatePackage, progress: (Long, Long) -> Unit): File {
        val files = UpdateFiles.create(File(context.cacheDir, "updates"))
        val part = files.part
        val ready = files.ready
        var conn: HttpsURLConnection? = null
        try {
            conn = connection(update.url)
            require(conn.responseCode == 200)
            val length = conn.getHeaderField("Content-Length")?.toLongOrNull()
            require(length == null || length == update.size)
            val hash = MessageDigest.getInstance("SHA-256"); var size = 0L
            conn.inputStream.use { input -> part.outputStream().use { output ->
                val bytes = ByteArray(65536)
                while (true) { checkActive(); val n = input.read(bytes); if (n < 0) break; size += n; require(size <= update.size); hash.update(bytes, 0, n); output.write(bytes, 0, n); progress(size, update.size) }
            } }
            if (size != update.size || hex(hash.digest()) != update.digest) throw LocalFailure("更新包大小或摘要不符，请重新下载")
            verifyPackage(part, update)
            checkActive(); check(part.renameTo(ready))
            return ready
        } finally { conn?.disconnect(); connection = null; part.delete() }
    }
    @Suppress("DEPRECATION")
    fun verifyPackage(file: File, update: UpdatePackage) {
        require(file.length() == update.size)
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input -> val b = ByteArray(65536); while (true) { checkActive(); val n = input.read(b); if (n < 0) break; digest.update(b, 0, n) } }
        require(hex(digest.digest()) == update.digest)
        val pm = context.packageManager
        val flags = if (Build.VERSION.SDK_INT >= 28) PackageManager.GET_SIGNING_CERTIFICATES else PackageManager.GET_SIGNATURES
        val candidate = pm.getPackageArchiveInfo(file.absolutePath, flags) ?: error("无效 APK")
        val installed = pm.getPackageInfo(context.packageName, flags)
        val code = if (Build.VERSION.SDK_INT >= 28) candidate.longVersionCode else candidate.versionCode.toLong()
        val installedCode = if (Build.VERSION.SDK_INT >= 28) installed.longVersionCode else installed.versionCode.toLong()
        fun signers(info: android.content.pm.PackageInfo): Set<String> =
            (if (Build.VERSION.SDK_INT >= 28) info.signingInfo?.apkContentsSigners else info.signatures)
                ?.map { hex(MessageDigest.getInstance("SHA-256").digest(it.toByteArray())) }?.toSet() ?: emptySet()
        verifyIdentity(
            PackageIdentity(installed.packageName, installed.versionName, installedCode, signers(installed)),
            PackageIdentity(candidate.packageName, candidate.versionName, code, signers(candidate)),
            update.version,
        )
    }
    companion object {
        fun verifyIdentity(installed: PackageIdentity, candidate: PackageIdentity, version: String) {
            if (candidate.name != installed.name || candidate.version != version) throw LocalFailure("安装包包名或版本与清单不一致")
            if (candidate.code != versionCode(version) || candidate.code <= installed.code) throw LocalFailure("安装包版本号无效或不高于已安装版本")
            if (installed.signers.isEmpty() || candidate.signers != installed.signers) throw LocalFailure("安装包签名不一致；调试版不能直接升级为正式签名版")
        }
        fun versionCode(value: String): Long {
            require(Regex("(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)").matches(value))
            val p = value.split('.').map { it.toLong() }
            require(p[0] <= 2100 && p[1] < 1000 && p[2] < 1000)
            return (p[0] * 1000000 + p[1] * 1000 + p[2]).also { require(it in 1..2100000000) }
        }
        fun parse(server: String, json: JSONObject, current: String): UpdatePackage? {
            val origin = Api.validServer(server)
            val version = json.getString("version")
            if (versionCode(version) <= versionCode(current)) return null
            val name = "vpn-android-universal-$version.apk"
            val items = json.optJSONArray("downloads") ?: throw LocalFailure("此版本尚无 Android 安装包")
            val matches = (0 until items.length()).map { items.getJSONObject(it) }.filter { it.optString("name") == name }
            if (matches.isEmpty()) throw LocalFailure("此版本尚无 Android 安装包")
            require(matches.size == 1) { "重复 Android 安装包" }
            val item = matches.single(); val url = URI(item.getString("url")); val base = URI(origin)
            require(url.scheme == "https" && url.host.equals(base.host, true) && effectivePort(url) == effectivePort(base) && url.rawUserInfo == null && url.rawQuery == null && url.rawFragment == null)
            require(Regex("/updates/releases/[0-9a-fA-F-]{36}/${Regex.escape(name)}").matches(url.rawPath))
            val size = item.getLong("size"); require(size in 1..268435456)
            val digest = item.getString("digest"); require(Regex("sha256:[0-9a-fA-F]{64}").matches(digest))
            return UpdatePackage(origin, version, name, url.toString(), size, digest.removePrefix("sha256:").lowercase(), json.optString("notes").take(12000))
        }
        private fun effectivePort(uri: URI) = if (uri.port == -1) 443 else uri.port
        private fun hex(bytes: ByteArray) = bytes.joinToString("") { "%02x".format(it) }
    }
}
