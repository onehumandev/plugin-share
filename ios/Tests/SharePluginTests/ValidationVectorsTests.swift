import XCTest
@testable import ShareCore

/// Asserts ShareCore against the cross-platform vectors in tests/fixtures/validation-vectors.json.
final class ValidationVectorsTests: XCTestCase {
    private struct Vectors: Decodable {
        struct URLCase: Decodable {
            let url: String
            let valid: Bool
        }

        struct Base64Case: Decodable {
            let data: String
            let decodedBytes: Int?
            let valid: Bool?
        }

        struct FilePathCase: Decodable {
            let path: String
            let posixValid: Bool
        }

        let urls: [URLCase]
        let base64: [Base64Case]
        let filePaths: [FilePathCase]
    }

    private func loadVectors() throws -> Vectors {
        // ios/Tests/SharePluginTests/<file> -> repo root
        let repoRoot = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
        let fixtureURL = repoRoot.appendingPathComponent("tests/fixtures/validation-vectors.json")
        let data = try Data(contentsOf: fixtureURL)
        return try JSONDecoder().decode(Vectors.self, from: data)
    }

    func testURLVectors() throws {
        let vectors = try loadVectors()
        XCTAssertFalse(vectors.urls.isEmpty)
        for testCase in vectors.urls {
            XCTAssertEqual(isValidWebURL(testCase.url), testCase.valid, "url: \(testCase.url.debugDescription)")
            let result = validateShareOptions(ShareOptions(url: testCase.url))
            XCTAssertEqual(result == nil, testCase.valid, "validateShareOptions url: \(testCase.url.debugDescription)")
        }
    }

    func testBase64Vectors() throws {
        let vectors = try loadVectors()
        XCTAssertFalse(vectors.base64.isEmpty)
        for testCase in vectors.base64 {
            let estimated = estimateBase64DecodedSize(testCase.data)
            if let expected = testCase.decodedBytes {
                XCTAssertEqual(estimated, expected, "base64: \(testCase.data.debugDescription)")
                XCTAssertEqual(decodeBase64(testCase.data)?.count, expected, "decode: \(testCase.data.debugDescription)")
            } else {
                XCTAssertEqual(testCase.valid, false, "malformed vector: \(testCase.data.debugDescription)")
                XCTAssertNil(estimated, "base64: \(testCase.data.debugDescription)")
            }
        }
    }

    func testFilePathVectors() throws {
        let vectors = try loadVectors()
        XCTAssertFalse(vectors.filePaths.isEmpty)
        for testCase in vectors.filePaths {
            XCTAssertEqual(isValidFilePath(testCase.path), testCase.posixValid, "path: \(testCase.path.debugDescription)")
            let result = validateShareOptions(ShareOptions(filePaths: [testCase.path]))
            XCTAssertEqual(result == nil, testCase.posixValid, "validateShareOptions path: \(testCase.path.debugDescription)")
        }
    }

    func testExistingRegularFileCheck() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("share-vectors-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let file = directory.appendingPathComponent("report.txt")
        try Data("hello".utf8).write(to: file)

        XCTAssertTrue(isExistingRegularFile(atPath: file.path))
        XCTAssertFalse(isExistingRegularFile(atPath: directory.path))
        XCTAssertFalse(isExistingRegularFile(atPath: directory.appendingPathComponent("missing.txt").path))
    }
}
