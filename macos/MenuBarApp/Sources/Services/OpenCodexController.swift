import AppKit
import Combine
import Foundation
import OSLog

@MainActor
final class OpenCodexController: ObservableObject {
    static let shared = OpenCodexController()

    @Published private(set) var status: OCXStatus = .unknown
    @Published private(set) var detectedPath: String?
    @Published private(set) var snapshot: OCXSnapshot?
    @Published private(set) var ocxVersion: String?
    @Published private(set) var updateState: OCXUpdateState = .idle

    private let logger = Logger(subsystem: "com.aieink.dashboard.menubar", category: "OpenCodex")
    private let processRunner: ProcessRunning
    private let candidates: [String]
    private let versionRefreshInterval: TimeInterval = 15 * 60

    private var panelMonitorTask: Task<Void, Never>?
    private var statusTask: Task<Void, Never>?
    private var operationTask: Task<Void, Never>?
    private var operationGeneration = 0
    private var statusGeneration = 0
    private var isPanelVisible = false
    private var isDetecting = false
    private var lastVersionCheck: Date?

    init(processRunner: ProcessRunning = ProcessRunner()) {
        self.processRunner = processRunner
        candidates = [
            "/opt/homebrew/bin/ocx",
            "/usr/local/bin/ocx",
            "\(NSHomeDirectory())/.npm-global/bin/ocx",
            "\(NSHomeDirectory())/.local/bin/ocx"
        ]

        let saved = AppSettings.shared.ocxCustomPath
        if Self.isExecutable(saved) {
            detectedPath = saved
        }
    }

    // MARK: - Panel lifecycle

    func panelDidAppear() {
        guard !isPanelVisible else { return }
        isPanelVisible = true
        panelMonitorTask?.cancel()
        panelMonitorTask = Task { @MainActor [weak self] in
            guard let self else { return }
            await self.checkOnce()

            while !Task.isCancelled && self.isPanelVisible {
                do {
                    try await Task.sleep(nanoseconds: OCXOperationPolicy.panelPollIntervalNanoseconds)
                } catch {
                    return
                }
                guard OCXOperationPolicy.shouldContinuePanel(
                    isVisible: self.isPanelVisible,
                    taskIsCancelled: Task.isCancelled
                ) else { return }
                await self.checkOnce()
            }
        }
    }

    func panelDidDisappear() {
        isPanelVisible = false
        panelMonitorTask?.cancel()
        panelMonitorTask = nil
        statusTask?.cancel()
        statusTask = nil
    }

    // MARK: - Detection and status

    /// Performs one executable resolution and one `ocx status --json` check.
    /// There is no permanent OpenCodex polling task outside the visible panel.
    func checkOnce() async {
        guard operationTask == nil, !isDetecting, !updateState.isBusy else { return }

        if let currentTask = statusTask {
            await currentTask.value
            return
        }

        statusGeneration += 1
        let requestGeneration = statusGeneration
        status = .checking

        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            await self.performStatusCheck(generation: requestGeneration)
        }
        statusTask = task
        await task.value

        if requestGeneration == statusGeneration {
            statusTask = nil
        }
    }

    func detectExecutable() async {
        guard operationTask == nil, !isDetecting, !updateState.isBusy else { return }
        isDetecting = true
        defer { isDetecting = false }
        invalidateStatusTask()
        status = .checking

        guard let path = await resolveExecutable() else {
            detectedPath = nil
            snapshot = nil
            status = .notInstalled
            return
        }

        setDetectedPath(path)
        await refreshVersionIfNeeded(force: true)
        await checkStatus(path: path, generation: statusGeneration)
    }

    private func performStatusCheck(generation: Int) async {
        guard generation == statusGeneration, !Task.isCancelled else { return }

        guard let path = await resolveExecutable() else {
            guard generation == statusGeneration, !Task.isCancelled else { return }
            detectedPath = nil
            snapshot = nil
            status = .notInstalled
            return
        }

        guard generation == statusGeneration, !Task.isCancelled else { return }
        setDetectedPath(path)
        await refreshVersionIfNeeded()
        await checkStatus(path: path, generation: generation)
    }

    private func checkStatus(path: String, generation: Int) async {
        do {
            let result = try await processRunner.run(
                executable: path,
                arguments: ["status", "--json"],
                environment: processEnvironment,
                timeout: OCXOperationPolicy.statusTimeout
            )
            let nextSnapshot = try OCXSnapshot(jsonData: result.stdoutData)
            guard OCXOperationPolicy.shouldApplyStatus(
                requestGeneration: generation,
                currentGeneration: statusGeneration,
                operationActive: operationTask != nil
            ), !Task.isCancelled else { return }
            apply(nextSnapshot)
        } catch is CancellationError {
            return
        } catch let error as ProcessRunnerError {
            guard OCXOperationPolicy.shouldApplyStatus(
                requestGeneration: generation,
                currentGeneration: statusGeneration,
                operationActive: operationTask != nil
            ), !Task.isCancelled else { return }
            if case .launchFailed = error, !Self.isExecutable(path) {
                detectedPath = nil
                snapshot = nil
                status = .notInstalled
            } else {
                snapshot = nil
                status = .unhealthy
            }
            logger.error("OpenCodex status check failed: \(String(describing: error))")
        } catch {
            guard OCXOperationPolicy.shouldApplyStatus(
                requestGeneration: generation,
                currentGeneration: statusGeneration,
                operationActive: operationTask != nil
            ), !Task.isCancelled else { return }
            snapshot = nil
            status = .unhealthy
            logger.error("OpenCodex status JSON failed: \(String(describing: error))")
        }
    }

    private func resolveExecutable() async -> String? {
        if let detectedPath, Self.isExecutable(detectedPath) {
            return detectedPath
        }

        let saved = AppSettings.shared.ocxCustomPath
        if Self.isExecutable(saved) {
            return saved
        }

        for candidate in candidates where Self.isExecutable(candidate) {
            return candidate
        }

        do {
            let result = try await processRunner.run(
                executable: "/bin/zsh",
                arguments: ["-lc", "command -v ocx"],
                environment: processEnvironment,
                timeout: 5
            )
            let path = OCXVersionParser.parse(result.stdout)
            if let path, Self.isExecutable(path) {
                return path
            }
        } catch {
            logger.debug("Unable to resolve ocx from login shell: \(String(describing: error))")
        }

        return nil
    }

    @discardableResult
    private func refreshVersionIfNeeded(force: Bool = false) async -> String? {
        guard let path = detectedPath else { return nil }
        let now = Date()
        if !force,
           let lastVersionCheck,
           now.timeIntervalSince(lastVersionCheck) < versionRefreshInterval {
            return ocxVersion
        }

        lastVersionCheck = now
        do {
            let result = try await processRunner.run(
                executable: path,
                arguments: ["--version"],
                environment: processEnvironment,
                timeout: 5
            )
            if let version = OCXVersionParser.parse(result.stdout) {
                ocxVersion = version
                return version
            }
        } catch {
            logger.debug("Unable to read ocx version: \(String(describing: error))")
        }
        return nil
    }

    // MARK: - Update management

    func checkForUpdate() async {
        guard operationTask == nil, !isDetecting, !updateState.isBusy else { return }

        operationGeneration += 1
        let generation = operationGeneration
        invalidateStatusTask()
        updateState = .checking

        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            await self.performUpdateCheck(generation: generation)
        }
        operationTask = task
        await task.value

        if operationGeneration == generation {
            operationTask = nil
        }
    }

    func updateOpenCodex() async {
        guard case .available = updateState, operationTask == nil, !isDetecting else { return }
        guard let oldVersion = OCXVersionParser.semanticVersion(from: ocxVersion ?? "")?.description else {
            updateState = .failed("Unable to read OpenCodex version")
            return
        }

        let wasRunning = status == .running
        operationGeneration += 1
        let generation = operationGeneration
        invalidateStatusTask()
        updateState = .updating

        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            await self.performUpdate(
                from: oldVersion,
                wasRunning: wasRunning,
                generation: generation
            )
        }
        operationTask = task
        await task.value

        if operationGeneration == generation {
            operationTask = nil
        }
    }

    private func performUpdateCheck(generation: Int) async {
        guard generation == operationGeneration, !Task.isCancelled else { return }

        guard let path = await resolveExecutable() else {
            detectedPath = nil
            snapshot = nil
            status = .notInstalled
            updateState = .failed("OpenCodex is not installed")
            return
        }

        guard generation == operationGeneration, !Task.isCancelled else { return }
        setDetectedPath(path)

        guard
            let currentOutput = await refreshVersionIfNeeded(force: true),
            let current = OCXVersionParser.semanticVersion(from: currentOutput)
        else {
            updateState = .failed("Unable to read OpenCodex version")
            return
        }

        let invocation = OCXCommandBuilder.updateCheck()
        do {
            let result = try await processRunner.run(
                executable: invocation.executable,
                arguments: invocation.arguments,
                environment: processEnvironment,
                timeout: OCXOperationPolicy.updateCheckTimeout
            )
            guard generation == operationGeneration, !Task.isCancelled else { return }

            guard let latest = OCXVersionParser.semanticVersion(from: result.stdout) else {
                updateState = .failed("Unable to check for updates")
                return
            }

            updateState = .checkResult(
                current: current.description,
                latest: latest.description
            )
        } catch is CancellationError {
            return
        } catch let error as ProcessRunnerError {
            if case .cancelled = error { return }
            logger.error("OpenCodex update check failed: \(String(describing: error))")
            updateState = .failed(Self.shortUpdateError(error, fallback: "Unable to check for updates"))
        } catch {
            logger.error("OpenCodex update check failed: \(String(describing: error))")
            updateState = .failed("Unable to check for updates")
        }
    }

    private func performUpdate(from oldVersion: String, wasRunning: Bool, generation: Int) async {
        guard generation == operationGeneration, !Task.isCancelled else { return }

        guard let path = await resolveExecutable() else {
            updateState = .failed("OpenCodex update failed")
            return
        }

        guard generation == operationGeneration, !Task.isCancelled else { return }
        setDetectedPath(path)

        let invocation = OCXCommandBuilder.update(ocxPath: path)
        do {
            _ = try await processRunner.run(
                executable: invocation.executable,
                arguments: invocation.arguments,
                environment: processEnvironment,
                timeout: OCXOperationPolicy.updateTimeout
            )
        } catch is CancellationError {
            return
        } catch let error as ProcessRunnerError {
            if case .cancelled = error { return }
            logger.error("OpenCodex update failed: \(String(describing: error))")
            updateState = .failed(Self.shortUpdateError(error, fallback: "OpenCodex update failed"))
            return
        } catch {
            logger.error("OpenCodex update failed: \(String(describing: error))")
            updateState = .failed("OpenCodex update failed")
            return
        }

        guard generation == operationGeneration, !Task.isCancelled else { return }
        guard let refreshedPath = await resolveExecutable() else {
            updateState = .failed("OpenCodex update failed")
            return
        }
        setDetectedPath(refreshedPath)
        let newVersionOutput = await refreshVersionIfNeeded(force: true)
        await refreshStatusAfterUpdate(path: refreshedPath, generation: generation)

        guard generation == operationGeneration, !Task.isCancelled else { return }
        updateState = .completion(
            from: oldVersion,
            to: newVersionOutput.flatMap { OCXVersionParser.semanticVersion(from: $0)?.description },
            restartRequired: wasRunning && status == .stopped
        )
    }

    private func refreshStatusAfterUpdate(path: String, generation: Int) async {
        do {
            let result = try await processRunner.run(
                executable: path,
                arguments: ["status", "--json"],
                environment: processEnvironment,
                timeout: OCXOperationPolicy.statusTimeout
            )
            let nextSnapshot = try OCXSnapshot(jsonData: result.stdoutData)
            guard generation == operationGeneration, !Task.isCancelled else { return }
            apply(nextSnapshot)
        } catch is CancellationError {
            return
        } catch {
            logger.error("OpenCodex status refresh after update failed: \(String(describing: error))")
        }
    }

    private static func shortUpdateError(_ error: Error, fallback: String) -> String {
        if let runnerError = error as? ProcessRunnerError,
           case .timedOut = runnerError {
            return "Command timed out"
        }
        return fallback
    }

    // MARK: - Manual lifecycle

    func ensure() async {
        await runOperation(.ensure)
    }

    func stop() async {
        await runOperation(.stop)
    }

    @discardableResult
    func openDashboard() -> Bool {
        guard status == .running, let url = snapshot?.dashboardURL else { return false }
        return NSWorkspace.shared.open(url)
    }

    var dashboardURL: URL? {
        guard status == .running else { return nil }
        return snapshot?.dashboardURL
    }

    var knownPort: Int? {
        snapshot?.port
    }

    var controlsBusy: Bool {
        status.isBusy || updateState.isBusy
    }

    private enum Operation {
        case ensure
        case stop

        var command: String {
            switch self {
            case .ensure: return "ensure"
            case .stop: return "stop"
            }
        }

        var expectedStatus: OCXStatus {
            switch self {
            case .ensure: return .running
            case .stop: return .stopped
            }
        }

        var transientStatus: OCXStatus {
            switch self {
            case .ensure: return .starting
            case .stop: return .stopping
            }
        }
    }

    private func runOperation(_ operation: Operation) async {
        guard !isDetecting, !updateState.isBusy else { return }
        if let existingTask = operationTask {
            existingTask.cancel()
            await existingTask.value
        }

        operationGeneration += 1
        let generation = operationGeneration
        invalidateStatusTask()
        status = operation.transientStatus

        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            await self.performOperation(operation, generation: generation)
        }
        operationTask = task
        await task.value

        if operationGeneration == generation {
            operationTask = nil
        }
    }

    private func performOperation(_ operation: Operation, generation: Int) async {
        guard let path = await resolveExecutable() else {
            guard generation == operationGeneration else { return }
            detectedPath = nil
            snapshot = nil
            status = .notInstalled
            return
        }

        guard generation == operationGeneration, !Task.isCancelled else { return }
        setDetectedPath(path)

        let invocation = OCXCommandBuilder.lifecycle(command: operation.command, ocxPath: path)
        var environment = processEnvironment
        for (key, value) in invocation.environmentOverrides {
            environment[key] = value
        }

        do {
            _ = try await processRunner.run(
                executable: invocation.executable,
                arguments: invocation.arguments,
                environment: environment,
                timeout: OCXOperationPolicy.operationTimeout
            )
        } catch is CancellationError {
            return
        } catch {
            logger.error("OpenCodex \(operation.command) failed: \(String(describing: error))")
        }

        var lastSnapshot: OCXSnapshot?
        for delay in OCXOperationPolicy.confirmationDelays {
            do {
                try await Task.sleep(nanoseconds: delay)
            } catch {
                return
            }

            guard generation == operationGeneration, !Task.isCancelled else { return }

            do {
                let result = try await processRunner.run(
                    executable: path,
                    arguments: ["status", "--json"],
                    environment: processEnvironment,
                    timeout: OCXOperationPolicy.statusTimeout
                )
                let nextSnapshot = try OCXSnapshot(jsonData: result.stdoutData)
                lastSnapshot = nextSnapshot

                if OCXOperationPolicy.reachedTarget(
                    nextSnapshot.resolvedStatus,
                    target: operation.expectedStatus
                ) {
                    apply(nextSnapshot)
                    return
                }
            } catch is CancellationError {
                return
            } catch {
                // Keep the transient state while confirmation is still in
                // progress. A later probe can still observe the target state.
            }
        }

        guard generation == operationGeneration, !Task.isCancelled else { return }
        if let lastSnapshot {
            apply(lastSnapshot)
        } else {
            snapshot = nil
            status = .unhealthy
        }
    }

    private func apply(_ nextSnapshot: OCXSnapshot) {
        snapshot = nextSnapshot
        status = nextSnapshot.resolvedStatus
    }

    private func setDetectedPath(_ path: String) {
        detectedPath = path
        if AppSettings.shared.ocxCustomPath != path {
            AppSettings.shared.ocxCustomPath = path
        }
    }

    private func invalidateStatusTask() {
        statusGeneration += 1
        statusTask?.cancel()
        statusTask = nil
    }

    private var processEnvironment: [String: String] {
        var environment = ProcessInfo.processInfo.environment
        var path = environment["PATH"] ?? ""
        path = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:\(path)"
        environment["PATH"] = path
        environment["HOME"] = NSHomeDirectory()
        environment["SHELL"] = "/bin/zsh"
        return environment
    }

    private static func isExecutable(_ path: String) -> Bool {
        !path.isEmpty && FileManager.default.isExecutableFile(atPath: path)
    }
}
