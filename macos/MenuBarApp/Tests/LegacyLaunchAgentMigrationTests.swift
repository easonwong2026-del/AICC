import Foundation
import Darwin
import XCTest
@testable import AICCCore

final class LegacyLaunchAgentMigrationTests: XCTestCase {
    private let legacyLabels = [
        "com.aieink.dashboard",
        "com.aieink.workbuddy-monitor",
        "com.aieink.log-maintenance",
    ]

    func testNoLegacyServicesLeavesUnrelatedPlistUntouched() throws {
        let home = try makeTemporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let unrelated = try makePlist(named: "com.example.other.plist", in: home)
        var commands = [[String]]()

        LegacyLaunchAgentMigration.run(homeDirectory: home) { arguments in
            commands.append(arguments)
            return false
        }

        XCTAssertTrue(FileManager.default.fileExists(atPath: unrelated.path))
        XCTAssertFalse(commands.contains { $0.first == "bootout" })
    }

    func testLoadedLegacyServicesAreUnloadedAndRemovedWithoutTouchingOtherPlists() throws {
        let home = try makeTemporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let launchAgents = home.appendingPathComponent("Library/LaunchAgents", isDirectory: true)
        try FileManager.default.createDirectory(at: launchAgents, withIntermediateDirectories: true)
        for label in legacyLabels {
            _ = try makePlist(named: "\(label).plist", in: home)
        }
        let unrelated = try makePlist(named: "com.example.other.plist", in: home)
        var loaded = Set(legacyLabels)
        var commands = [[String]]()

        LegacyLaunchAgentMigration.run(homeDirectory: home) { arguments in
            commands.append(arguments)
            guard arguments.count >= 2 else { return false }
            let label = arguments.last?.split(separator: "/").last.map(String.init)
            switch arguments[0] {
            case "print":
                return label.map(loaded.contains) ?? false
            case "bootout":
                if let label, loaded.remove(label) != nil {
                    return true
                }
                return false
            default:
                return false
            }
        }

        for label in legacyLabels {
            XCTAssertFalse(
                FileManager.default.fileExists(atPath: launchAgents.appendingPathComponent("\(label).plist").path)
            )
            XCTAssertTrue(commands.contains(["bootout", "gui/\(getuid())/\(label)"]))
        }
        XCTAssertTrue(FileManager.default.fileExists(atPath: unrelated.path))
    }

    func testStaleLegacyPlistIsRemovedWhenJobIsNotLoaded() throws {
        let home = try makeTemporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let plist = try makePlist(named: "com.aieink.log-maintenance.plist", in: home)

        LegacyLaunchAgentMigration.run(homeDirectory: home) { _ in false }

        XCTAssertFalse(FileManager.default.fileExists(atPath: plist.path))
    }

    func testUnloadFailureDoesNotThrowOrRemoveStillLoadedService() throws {
        let home = try makeTemporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let plist = try makePlist(named: "com.aieink.dashboard.plist", in: home)
        var commands = [[String]]()

        LegacyLaunchAgentMigration.run(homeDirectory: home) { arguments in
            commands.append(arguments)
            return arguments.first == "print"
        }

        XCTAssertTrue(FileManager.default.fileExists(atPath: plist.path))
        XCTAssertTrue(commands.contains { $0.first == "bootout" })
    }

    func testRepeatedRunIsIdempotent() throws {
        let home = try makeTemporaryHome()
        defer { try? FileManager.default.removeItem(at: home) }
        let plist = try makePlist(named: "com.aieink.dashboard.plist", in: home)
        var loaded = Set(["com.aieink.dashboard"])

        let launchctl: ([String]) -> Bool = { arguments in
            guard arguments.count >= 2 else { return false }
            let label = arguments.last?.split(separator: "/").last.map(String.init)
            switch arguments[0] {
            case "print":
                return label.map(loaded.contains) ?? false
            case "bootout":
                if let label, loaded.remove(label) != nil {
                    return true
                }
                return false
            default:
                return false
            }
        }

        LegacyLaunchAgentMigration.run(homeDirectory: home, launchctl: launchctl)
        LegacyLaunchAgentMigration.run(homeDirectory: home, launchctl: launchctl)

        XCTAssertFalse(FileManager.default.fileExists(atPath: plist.path))
        XCTAssertTrue(loaded.isEmpty)
    }

    private func makeTemporaryHome() throws -> URL {
        let home = FileManager.default.temporaryDirectory
            .appendingPathComponent("aicc-legacy-migration-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        return home
    }

    private func makePlist(named name: String, in home: URL) throws -> URL {
        let directory = home.appendingPathComponent("Library/LaunchAgents", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = directory.appendingPathComponent(name)
        try Data("legacy".utf8).write(to: url)
        return url
    }
}
