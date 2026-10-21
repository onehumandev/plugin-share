package plugin.vnidrop.share

internal data class ShareIntentPayload(
    val title: String?,
    val body: String?,
) {
    companion object {
        fun from(options: ShareOptions): ShareIntentPayload {
            val text = options.text.nonEmptyOrNull()
            val title = options.title.nonEmptyOrNull()
            val url = options.url.nonEmptyOrNull()

            val body = if (text != null && url != null) "$text\n$url" else text ?: url

            return ShareIntentPayload(
                title = title,
                body = body,
            )
        }

        private fun String?.nonEmptyOrNull(): String? = this?.takeIf { it.isNotEmpty() }
    }
}
