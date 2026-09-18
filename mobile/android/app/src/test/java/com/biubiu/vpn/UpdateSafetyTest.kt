package com.biubiu.vpn

import org.junit.Assert.*
import org.junit.Test
import java.nio.file.Files

class UpdateSafetyTest {
    @Test fun oldDownloadCleanupCannotRemoveNewDownloadOrInstallerFile() {
        val directory = Files.createTempDirectory("update-safety").toFile()
        try {
            val old = UpdateFiles.create(directory)
            val next = UpdateFiles.create(directory)
            next.part.writeText("verified payload")
            assertTrue(old.part.delete())
            assertEquals("verified payload", next.part.readText())
            assertTrue(next.part.renameTo(next.ready))
            val third = UpdateFiles.create(directory)
            third.part.delete()
            assertEquals("verified payload", next.ready.readText())
            assertEquals(next.ready, UpdateFiles.forPath(directory, "/${next.ready.name}"))
            for (path in listOf("/../vault", "/candidate.part", "/${next.part.name}", "/nested/${next.ready.name}")) {
                try { UpdateFiles.forPath(directory, path); fail("must not expose $path") } catch (_: IllegalArgumentException) {}
            }
        } finally { directory.deleteRecursively() }
    }
    @Test fun installRequiresExactIdentityAndNewerVersion() {
        val installed = PackageIdentity("com.biubiu.vpn", "0.1.33", 1033, setOf("production"))
        val valid = PackageIdentity("com.biubiu.vpn", "0.1.34", 1034, setOf("production"))
        ClientUpdates.verifyIdentity(installed, valid, "0.1.34")
        for (invalid in listOf(
            valid.copy(name = "other.app"), valid.copy(version = "9.0.0"), valid.copy(code = 1033),
            valid.copy(code = 1035), valid.copy(signers = emptySet()), valid.copy(signers = setOf("debug")),
            valid.copy(signers = setOf("production", "extra")),
        )) {
            try { ClientUpdates.verifyIdentity(installed, invalid, "0.1.34"); fail("must reject $invalid") } catch (_: LocalFailure) {}
        }
        try { ClientUpdates.verifyIdentity(installed, installed, "0.1.33"); fail("must reject same version") } catch (_: LocalFailure) {}
    }
}
