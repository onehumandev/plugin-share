#if canImport(UIKit)
import Tauri
import ShareCore
import UIKit
import WebKit

@_cdecl("init_plugin_share")
public func initPlugin() -> Plugin {
    return SharePlugin()
}

public class SharePlugin: Plugin {

    private var webview: WKWebView?

    // Tauri invokes plugin commands on a background IPC queue, while the share sheet
    // completion runs on the main thread. All access to the share state below must go
    // through `stateLock`.
    private let stateLock = NSLock()
    private var temporaryFileURLs: [URL] = []
    private var shareInProgress = false

    open override func load(webview: WKWebView) {
        self.webview = webview
    }

    @objc func canShare(_ invoke: Invoke) throws {
        // The native share sheet is always available on iOS.
        invoke.resolve(["value": true])
    }

    /**
     * Deletes all temporary files created by this plugin.
     * This is a manual cleanup utility for the developer.
     */
    @objc func cleanup(_ invoke: Invoke) {
        stateLock.lock()
        if shareInProgress {
            stateLock.unlock()
            invoke.reject("Cannot cleanup while sharing is in progress.")
            return
        }

        // Holding the lock prevents a share from starting while the directory is removed.
        let result = Result<Void, Error> {
            let shareDir = try getSafeShareDir()
            if FileManager.default.fileExists(atPath: shareDir.path) {
                try FileManager.default.removeItem(at: shareDir)
            }
            temporaryFileURLs.removeAll()
        }
        stateLock.unlock()

        switch result {
        case .success:
            invoke.resolve()
        case .failure(let error):
            invoke.reject("Error during cleanup: \(error.localizedDescription)")
        }
    }

    @objc func share(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(ShareOptions.self)
        if let validationError = validateShareOptions(args) {
            invoke.reject(validationError)
            return
        }

        // Share local files directly from their original paths. This preserves the
        // original filename and avoids creating temporary copies.
        let filePaths = args.filePaths ?? []
        for path in filePaths where !isExistingRegularFile(atPath: path) {
            invoke.reject("File not found: \(path)")
            return
        }

        guard beginShare() else {
            invoke.reject("Share already in progress.")
            return
        }

        // Empty strings are treated as absent.
        let subject = nonEmpty(args.title)
        var activityItems: [Any] = []
        var createdFileURLs: [URL] = []

        if let urlString = nonEmpty(args.url), let url = URL(string: urlString) {
            activityItems.append(url)
        }
        if let text = nonEmpty(args.text) {
            activityItems.append(text)
        }

        // Decoding and writing happen on the IPC queue, off the main thread.
        if let files = args.files {
            for file in files {
                guard let decodedData = decodeBase64(file.data) else {
                    fail(invoke, "Invalid Base64 data for file: \(file.name)", files: createdFileURLs)
                    return
                }
                if decodedData.count > maxFileBytes {
                    fail(invoke, "File '\(file.name)' exceeds the maximum size of \(maxFileBytes) bytes.", files: createdFileURLs)
                    return
                }

                do {
                    let tempFileURL = try createSafeTempFile(for: file.name)
                    createdFileURLs.append(tempFileURL)
                    try decodedData.write(to: tempFileURL, options:.atomic)
                    activityItems.append(tempFileURL)
                } catch {
                    fail(invoke, "Failed to create temporary file: \(error.localizedDescription)", files: createdFileURLs)
                    return
                }
            }
        }

        for path in filePaths {
            activityItems.append(URL(fileURLWithPath: path))
        }

        if activityItems.isEmpty {
            fail(invoke, "No content provided to share.")
            return
        }

        registerTemporaryFiles(createdFileURLs)
        presentShareSheet(invoke: invoke, activityItems: activityItems, subject: subject, anchor: args.anchor)
    }

    /**
     * Returns the URL for a dedicated, secure directory for storing temporary share files.
     * Creates it if it doesn't exist.
     */
    private func getSafeShareDir() throws -> URL {
        let tempDir = FileManager.default.temporaryDirectory
        let shareDir = tempDir.appendingPathComponent("share")

        if !FileManager.default.fileExists(atPath: shareDir.path) {
            try FileManager.default.createDirectory(
                at: shareDir,
                withIntermediateDirectories: true
            )
        }

        return shareDir
    }

    /**
     * Creates a safe temporary file URL within the dedicated share directory.
     * It keeps only the last path component of the name and places the file in its own UUID directory.
     */
    private func createSafeTempFile(for untrustedFileName: String) throws -> URL {
        let safeDir = try getSafeShareDir()

        let sanitizedBaseName = URL(fileURLWithPath: untrustedFileName).lastPathComponent
        if sanitizedBaseName.isEmpty || sanitizedBaseName == "." || sanitizedBaseName == ".." || sanitizedBaseName == "/" {
            throw NSError(domain: "SharePlugin", code: 1, userInfo: [NSLocalizedDescriptionKey: "Invalid filename: sanitized name is empty."])
        }

        // Each file gets its own directory so the original filename is preserved
        // without colliding with other shared files.
        let fileDir = safeDir.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: fileDir, withIntermediateDirectories: false)

        return fileDir.appendingPathComponent(sanitizedBaseName)
    }

    private func presentShareSheet(invoke: Invoke, activityItems: [Any], subject: String?, anchor: ShareAnchor? = nil) {
        DispatchQueue.main.async {
            guard let viewController = self.manager.viewController else {
                self.fail(invoke, "Could not find root view controller.")
                return
            }

            guard viewController.presentedViewController == nil else {
                self.fail(invoke, "Another view controller is already being presented.")
                return
            }

            var items = activityItems
            // Provide the title as the subject (e.g. for Mail) via the leading text/url item.
            if let subject = subject, let first = items.first, first is URL || first is String,
               !((first as? URL)?.isFileURL ?? false) {
                items[0] = ShareSubjectItemSource(item: first, subject: subject)
            }

            let activityViewController = UIActivityViewController(activityItems: items, applicationActivities: nil)
            if UIDevice.current.userInterfaceIdiom == .pad {
                activityViewController.modalPresentationStyle = .popover
            }

            activityViewController.completionWithItemsHandler = { _, _, _, error in
                DispatchQueue.main.async {
                    if let anError = error {
                        self.fail(invoke, "Sharing failed: \(anError.localizedDescription)")
                    } else {
                        self.cleanupTemporaryFiles(self.resetShareState())
                        invoke.resolve()
                    }
                }
            }

            // iPad presentation logic
            if let popoverController = activityViewController.popoverPresentationController {
                guard let sourceView = self.webview ?? viewController.view else {
                    self.fail(invoke, "Could not find a source view for the share popover.")
                    return
                }
                popoverController.sourceView = sourceView

                if let anchor = anchor {
                    popoverController.sourceRect = CGRect(
                        x: anchor.x,
                        y: anchor.y,
                        width: anchor.width,
                        height: anchor.height
                    )
                    popoverController.permittedArrowDirections = [.up, .down]
                } else {
                    popoverController.sourceRect = CGRect(x: sourceView.bounds.midX, y: sourceView.bounds.midY, width: 0, height: 0)
                    popoverController.permittedArrowDirections = []
                }
            }

            viewController.present(activityViewController, animated: true, completion: nil)
        }
    }

    /**
     * Ends the current share with an error: deletes the given files and every
     * registered temporary file, resets the share state and rejects the invoke.
     */
    private func fail(_ invoke: Invoke, _ message: String, files: [URL] = []) {
        cleanupTemporaryFiles(files + resetShareState())
        invoke.reject(message)
    }

    /// Atomically marks a share as started. Returns false if one is already in progress.
    private func beginShare() -> Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        if shareInProgress {
            return false
        }
        shareInProgress = true
        return true
    }

    private func registerTemporaryFiles(_ urls: [URL]) {
        stateLock.lock()
        defer { stateLock.unlock() }
        temporaryFileURLs.append(contentsOf: urls)
    }

    private func resetShareState() -> [URL] {
        stateLock.lock()
        defer { stateLock.unlock() }
        let filesToClean = temporaryFileURLs
        temporaryFileURLs.removeAll()
        shareInProgress = false
        return filesToClean
    }

    private func cleanupTemporaryFiles(_ urls: [URL]) {
        if urls.isEmpty {
            return
        }
        DispatchQueue.global(qos:.utility).async {
            for url in urls {
                try? FileManager.default.removeItem(at: url)
                // Also remove the per-file directory created by createSafeTempFile.
                try? FileManager.default.removeItem(at: url.deletingLastPathComponent())
            }
        }
    }
}

/**
 * Wraps the leading text or URL item so the share title is offered as the subject
 * (e.g. the email subject line) while the shared item itself is unchanged.
 */
private final class ShareSubjectItemSource: NSObject, UIActivityItemSource {
    private let item: Any
    private let subject: String

    init(item: Any, subject: String) {
        self.item = item
        self.subject = subject
    }

    func activityViewControllerPlaceholderItem(_ activityViewController: UIActivityViewController) -> Any {
        return item
    }

    func activityViewController(_ activityViewController: UIActivityViewController, itemForActivityType activityType: UIActivity.ActivityType?) -> Any? {
        return item
    }

    func activityViewController(_ activityViewController: UIActivityViewController, subjectForActivityType activityType: UIActivity.ActivityType?) -> String {
        return subject
    }
}
#endif
