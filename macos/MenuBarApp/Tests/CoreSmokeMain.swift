import Foundation

enum CoreSmokeError: Error {
    case assertion(String)
}

@main
struct CoreSmokeMain {
    static func require(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        guard condition() else { throw CoreSmokeError.assertion(message) }
    }

    static func main() async throws {
        let fixtureRoot = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)

        func fixture(_ name: String) throws -> Data {
            try Data(contentsOf: fixtureRoot.appendingPathComponent(name))
        }

        let running = try OCXSnapshot(jsonData: fixture("ocx-running.json"))
        try require(running.resolvedStatus == .running, "running fixture")
        try require(running.port == 12_001, "dynamic port")
        try require(running.dashboardURL?.port == 12_001, "dashboard URL")

        let unhealthy = try OCXSnapshot(jsonData: fixture("ocx-running-missing-health.json"))
        try require(unhealthy.resolvedStatus == .unhealthy, "missing health")
        let stopped = try OCXSnapshot(jsonData: fixture("ocx-stopped.json"))
        try require(stopped.resolvedStatus == .stopped, "stopped fixture")
        do {
            _ = try OCXSnapshot(jsonData: fixture("ocx-bad-json.json"))
            throw CoreSmokeError.assertion("bad JSON accepted")
        } catch is OCXSnapshotError {
            // Expected.
        }

        let runner = ProcessRunner()
        let output = try await runner.run(
            executable: "/bin/sh",
            arguments: ["-c", "printf stdout; printf stderr 1>&2"],
            timeout: 2
        )
        try require(output.stdout == "stdout", "stdout capture")
        try require(output.stderr == "stderr", "stderr capture")

        do {
            _ = try await runner.run(
                executable: "/bin/sleep",
                arguments: ["5"],
                timeout: 0.1
            )
            throw CoreSmokeError.assertion("timeout did not fire")
        } catch let error as ProcessRunnerError {
            guard case .timedOut = error else { throw error }
        }

        print("AICC Swift core smoke tests passed.")
    }
}
