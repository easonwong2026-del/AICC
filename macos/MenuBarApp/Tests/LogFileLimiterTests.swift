import Foundation
import XCTest
@testable import AICCCore

final class LogFileLimiterTests: XCTestCase {
    func testOversizedLogKeepsOnlyRecentOutput() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("aicc-log-limiter-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let url = directory.appendingPathComponent("server.log")
        let marker = Data("latest output\n".utf8)
        var original = Data(repeating: 0x41, count: Int(LogFileLimiter.maxBytes))
        original.append(marker)
        try original.write(to: url)

        LogFileLimiter.trimIfNeeded(url)

        let trimmed = try Data(contentsOf: url)
        XCTAssertEqual(trimmed.count, Int(LogFileLimiter.retainedBytes))
        XCTAssertEqual(Data(trimmed.suffix(marker.count)), marker)
    }

    func testSmallLogIsUnchanged() throws {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("aicc-log-limiter-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }

        let url = directory.appendingPathComponent("server.log")
        let original = Data("keep this output\n".utf8)
        try original.write(to: url)

        LogFileLimiter.trimIfNeeded(url)

        XCTAssertEqual(try Data(contentsOf: url), original)
    }
}
