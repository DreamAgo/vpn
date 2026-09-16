package com.biubiu.vpn

import java.io.File
import java.util.UUID

/** Each transfer owns its files, including while an older Activity is unwinding. */
data class UpdateFiles(val part: File, val ready: File) {
    companion object {
        fun create(directory: File): UpdateFiles {
            check(directory.isDirectory || directory.mkdirs())
            val name = "candidate-${UUID.randomUUID()}"
            val part = File(directory, "$name.part")
            check(part.createNewFile())
            return UpdateFiles(part, File(directory, "$name.apk"))
        }
        fun forPath(directory: File, path: String): File {
            require(Regex("/candidate-[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\\.apk").matches(path))
            return File(directory, path.removePrefix("/"))
        }
    }
}
