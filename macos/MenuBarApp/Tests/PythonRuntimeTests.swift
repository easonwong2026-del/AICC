import XCTest
@testable import AICCCore

/// The bundled runtime is a product requirement: a release build must refuse to
/// borrow the host's `python3`, because the host's is 3.9 with LibreSSL and
/// cannot do TLS 1.3 at all.
final class PythonRuntimeTests: XCTestCase {
    private let bundle = URL(fileURLWithPath: "/Applications/AICC.app")

    private func locate(environment: [String: String], executable: [String]) -> PythonRuntime.Choice {
        let allowed = Set(executable)
        return PythonRuntime.locate(bundleURL: bundle, environment: environment) { allowed.contains($0) }
    }

    private var bundledPath: String {
        "/Applications/AICC.app/Contents/Resources/Python/bin/python3"
    }

    func testTheBundledRuntimeIsPreferred() {
        let choice = locate(environment: [:], executable: [bundledPath, "/usr/bin/python3"])
        XCTAssertEqual(choice, .bundled(bundledPath))
        XCTAssertEqual(choice.path, bundledPath)
        XCTAssertNil(choice.message)
    }

    func testAReleaseBuildWithoutTheRuntimeRefusesToStart() {
        // A host python3 exists and is executable — and must still be ignored.
        let choice = locate(environment: [:], executable: ["/usr/bin/python3", "/opt/homebrew/bin/python3"])
        XCTAssertNil(choice.path, "a release build must not fall back to the host interpreter")
        guard case .missing(let reason) = choice else {
            return XCTFail("expected missing, got \(choice)")
        }
        XCTAssertTrue(reason.contains("Resources/Python/bin/python3"), reason)
        XCTAssertTrue(reason.contains("不会退回系统 Python"), reason)
    }

    func testEnvironmentOverrideIsIgnoredOutsideDevelopment() {
        let choice = locate(
            environment: ["AICC_PYTHON_PATH": "/tmp/evil/python3"],
            executable: ["/tmp/evil/python3"]
        )
        XCTAssertNil(choice.path, "AICC_PYTHON_PATH must not work in a release build")
    }

    func testDevelopmentModeMayUseTheHost() {
        let choice = locate(
            environment: ["AICC_DEV": "1", "AICC_PYTHON_PATH": "/tmp/dev/python3"],
            executable: ["/tmp/dev/python3", "/usr/bin/python3"]
        )
        XCTAssertEqual(choice, .development("/tmp/dev/python3"))
    }

    func testDevelopmentModeFallsBackToKnownLocations() {
        let choice = locate(environment: ["AICC_DEV": "1"], executable: ["/usr/bin/python3"])
        XCTAssertEqual(choice, .development("/usr/bin/python3"))
    }

    func testDevelopmentModeWithNothingAvailableStillFails() {
        let choice = locate(environment: ["AICC_DEV": "1"], executable: [])
        XCTAssertNil(choice.path)
        XCTAssertNotNil(choice.message)
    }

    func testABrokenBundleEntryPointIsNotTrusted() {
        // The path exists in the bundle but is not executable (a truncated copy).
        let choice = locate(environment: [:], executable: [])
        XCTAssertNil(choice.path)
    }
}
