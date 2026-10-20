package plugin.vnidrop.share

import android.content.Context
import androidx.core.content.FileProvider

/**
 * Plugin-owned FileProvider subclass. Using a dedicated class and authority avoids
 * manifest merge collisions with a host app that declares the stock
 * androidx FileProvider under `${applicationId}.fileprovider`.
 */
class ShareFileProvider : FileProvider() {
    companion object {
        private const val AUTHORITY_SUFFIX = ".vnidrop.share.fileprovider"

        fun authority(context: Context): String = context.packageName + AUTHORITY_SUFFIX
    }
}
