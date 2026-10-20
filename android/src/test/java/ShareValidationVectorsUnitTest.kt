package plugin.vnidrop.share

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.File

/** Asserts the shared cross-platform vectors in tests/fixtures/validation-vectors.json. */
class ShareValidationVectorsUnitTest {
    private val vectors: JSONObject by lazy {
        val file = File("../tests/fixtures/validation-vectors.json")
        assertTrue("Missing shared vectors at ${file.absolutePath}", file.isFile)
        JSONObject(file.readText(Charsets.UTF_8))
    }

    @Test
    fun urlVectors() {
        val urls = vectors.getJSONArray("urls")
        assertTrue(urls.length() > 0)
        for (i in 0 until urls.length()) {
            val case = urls.getJSONObject(i)
            val url = case.getString("url")
            assertEquals("url: ${escape(url)}", case.getBoolean("valid"), ShareValidation.isValidWebUrl(url))
        }
    }

    @Test
    fun base64Vectors() {
        val cases = vectors.getJSONArray("base64")
        assertTrue(cases.length() > 0)
        for (i in 0 until cases.length()) {
            val case = cases.getJSONObject(i)
            val data = case.getString("data")
            if (case.has("decodedBytes")) {
                assertEquals(
                    "base64: ${escape(data)}",
                    case.getLong("decodedBytes"),
                    ShareValidation.estimateBase64DecodedSize(data)
                )
            } else {
                assertEquals(false, case.getBoolean("valid"))
                try {
                    ShareValidation.estimateBase64DecodedSize(data)
                    fail("Expected invalid base64: ${escape(data)}")
                } catch (_: SecurityException) {
                    // expected
                }
            }
        }
    }

    @Test
    fun filePathVectors() {
        val cases = vectors.getJSONArray("filePaths")
        assertTrue(cases.length() > 0)
        for (i in 0 until cases.length()) {
            val case = cases.getJSONObject(i)
            val path = case.getString("path")
            val expected = case.getBoolean("posixValid")
            assertEquals("filePath: ${escape(path)}", expected, ShareValidation.isValidFilePath(path))

            val options = ShareOptions().apply { filePaths = listOf(path) }
            val accepted = try {
                ShareValidation.validateShareOptions(options)
                true
            } catch (_: SecurityException) {
                false
            }
            assertEquals("validateShareOptions filePath: ${escape(path)}", expected, accepted)
        }
    }

    private fun escape(value: String): String = value.map {
        if (it.code < 0x20 || it.code in 0x7f..0xa0 || it.code > 0x7e) "\\u%04x".format(it.code) else it.toString()
    }.joinToString("")
}
