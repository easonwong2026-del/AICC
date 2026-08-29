@testable import AICCCore
import Foundation
import XCTest

final class ProcessRunnerTests: XCTestCase {
    private let runner = ProcessRunner()

    func testCapturesStdoutAndStderrText() async throws {
        let result = try await runner.run(
            executable: "/bin/sh",
            arguments: ["-c", "printf stdout; printf stderr 1>&2"],
            timeout: 2
        )
        XCTAssertEqual(result.stdout, "stdout")
        XCTAssertEqual(result.stderr, "stderr")
    }

    func testConcurrentStdoutAndStderrDrain() async throws {
        let result = try await runner.run(
            executable: "/bin/sh",
            arguments: ["-c", "head -c 200000 /dev/zero; head -c 200000 /dev/zero 1>&2"],
            timeout: 5
        )
        XCTAssertEqual(result.terminationStatus, 0)
        XCTAssertEqual(result.stdoutData.count, 200_000)
        XCTAssertEqual(result.stderrData.count, 200_000)
    }

    func testNonZeroExitPreservesDiagnostics() async {
        do {
            _ = try await runner.run(
                executable: "/bin/sh",
                arguments: ["-c", "printf failure 1>&2; exit 7"],
                timeout: 5
            )
            XCTFail("Expected a non-zero exit error")
        } catch let error as ProcessRunnerError {
            guard case .nonZeroExit(let status, _, let stderr) = error else {
                return XCTFail("Unexpected ProcessRunnerError: \(error)")
            }
            XCTAssertEqual(status, 7)
            XCTAssertEqual(stderr, "failure")
        } catch {
            XCTFail("Unexpected error: \(error)")
        }
    }

    func testTimeoutTerminatesProcess() async {
        do {
            _ = try await runner.run(
                executable: "/bin/sleep",
                arguments: ["10"],
                timeout: 0.1
            )
            XCTFail("Expected a timeout")
        } catch let error as ProcessRunnerError {
            guard case .timedOut = error else {
                return XCTFail("Unexpected ProcessRunnerError: \(error)")
            }
        } catch {
            XCTFail("Unexpected error: \(error)")
        }
    }

    func testCancellationTerminatesProcess() async {
        let task = Task {
            try await runner.run(
                executable: "/bin/sleep",
                arguments: ["10"],
                timeout: 10
            )
        }
        task.cancel()

        do {
            _ = try await task.value
            XCTFail("Expected cancellation")
        } catch is CancellationError {
            // Accept Swift task cancellation before the runner starts.
        } catch let error as ProcessRunnerError {
            guard case .cancelled = error else {
                return XCTFail("Unexpected ProcessRunnerError: \(error)")
            }
        } catch {
            XCTFail("Unexpected error: \(error)")
        }
    }

    func testImmediateCancellationAfterCreationReturnsQuickly() async {
        for _ in 0..<12 {
            let startedAt = Date()
            let task = Task {
                try await runner.run(
                    executable: "/bin/sleep",
                    arguments: ["10"],
                    timeout: 2
                )
            }
            await Task.yield()
            task.cancel()

            do {
                _ = try await task.value
                XCTFail("Expected cancellation")
            } catch is CancellationError {
                // Cancellation before ProcessRunner starts is also valid.
            } catch let error as ProcessRunnerError {
                guard case .cancelled = error else {
                    XCTFail("Unexpected ProcessRunnerError: \(error)")
                    continue
                }
            } catch {
                XCTFail("Unexpected error: \(error)")
            }

            XCTAssertLessThan(
                Date().timeIntervalSince(startedAt),
                1.5,
                "Cancellation must not wait for the full process timeout."
            )
        }
    }

    func testMissingExecutableIsAProcessFailure() async {
        do {
            _ = try await runner.run(
                executable: "/definitely/not/an/ocx",
                arguments: ["status", "--json"],
                timeout: 5
            )
            XCTFail("Expected launch failure")
        } catch let error as ProcessRunnerError {
            guard case .launchFailed = error else {
                return XCTFail("Unexpected ProcessRunnerError: \(error)")
            }
        } catch {
            XCTFail("Unexpected error: \(error)")
        }
    }
}
