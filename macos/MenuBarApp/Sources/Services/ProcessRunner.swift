import Foundation
import Darwin

enum ProcessRunnerError: Error, LocalizedError, Equatable {
    case cancelled
    case timedOut(TimeInterval)
    case launchFailed(String)
    case nonZeroExit(status: Int32, stdout: String, stderr: String)

    var errorDescription: String? {
        switch self {
        case .cancelled:
            return "Process cancelled"
        case .timedOut(let timeout):
            return String(format: "Process timed out after %.1f seconds", timeout)
        case .launchFailed(let message):
            return "Unable to launch process: \(message)"
        case .nonZeroExit(let status, _, let stderr):
            let detail = stderr.trimmingCharacters(in: .whitespacesAndNewlines)
            return detail.isEmpty ? "Process exited with status \(status)" : detail
        }
    }
}

struct ProcessRunnerResult: Equatable {
    let terminationStatus: Int32
    let stdoutData: Data
    let stderrData: Data

    var stdout: String {
        String(decoding: stdoutData, as: UTF8.self)
    }

    var stderr: String {
        String(decoding: stderrData, as: UTF8.self)
    }
}

/// Runs a child process away from the MainActor.
///
/// Both output streams are drained concurrently before waiting for the child
/// to finish. A single locked completion path prevents cancellation, timeout,
/// launch failure, and normal termination from resuming a continuation twice.
protocol ProcessRunning {
    func run(
        executable: String,
        arguments: [String],
        environment: [String: String]?,
        currentDirectoryURL: URL?,
        timeout: TimeInterval
    ) async throws -> ProcessRunnerResult
}

extension ProcessRunning {
    func run(
        executable: String,
        arguments: [String] = [],
        environment: [String: String]? = nil,
        timeout: TimeInterval
    ) async throws -> ProcessRunnerResult {
        try await run(
            executable: executable,
            arguments: arguments,
            environment: environment,
            currentDirectoryURL: nil,
            timeout: timeout
        )
    }
}

struct ProcessRunner: ProcessRunning {
    func run(
        executable: String,
        arguments: [String] = [],
        environment: [String: String]? = nil,
        currentDirectoryURL: URL? = nil,
        timeout: TimeInterval
    ) async throws -> ProcessRunnerResult {
        try Task.checkCancellation()

        let execution = ProcessExecution()
        return try await withTaskCancellationHandler(operation: {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<ProcessRunnerResult, Error>) in
                execution.start(
                    executable: executable,
                    arguments: arguments,
                    environment: environment,
                    currentDirectoryURL: currentDirectoryURL,
                    timeout: timeout
                ) { result in
                    continuation.resume(with: result)
                }
            }
        }, onCancel: {
            execution.cancel()
        })
    }

    private final class DataBox: @unchecked Sendable {
        private let lock = NSLock()
        private var value = Data()

        func set(_ data: Data) {
            lock.lock()
            value = data
            lock.unlock()
        }

        func get() -> Data {
            lock.lock()
            defer { lock.unlock() }
            return value
        }
    }

    private final class ProcessExecution: @unchecked Sendable {
        private let stateLock = NSLock()
        private let queue = DispatchQueue.global(qos: .utility)

        private var process: Process?
        private var completion: ((Result<ProcessRunnerResult, Error>) -> Void)?
        private var timeoutWorkItem: DispatchWorkItem?
        private var forceKillWorkItem: DispatchWorkItem?
        private var terminalError: ProcessRunnerError?
        private var completed = false
        private var cancellationRequested = false

        func start(
            executable: String,
            arguments: [String],
            environment: [String: String]?,
            currentDirectoryURL: URL?,
            timeout: TimeInterval,
            completion: @escaping (Result<ProcessRunnerResult, Error>) -> Void
        ) {
            stateLock.lock()
            self.completion = completion
            let cancelledBeforeStart = cancellationRequested
            stateLock.unlock()

            if cancelledBeforeStart {
                finish(.failure(ProcessRunnerError.cancelled))
                return
            }

            queue.async { [weak self] in
                guard let self else { return }

                let child = Process()
                let stdoutPipe = Pipe()
                let stderrPipe = Pipe()
                child.executableURL = URL(fileURLWithPath: executable)
                child.arguments = arguments
                child.standardOutput = stdoutPipe
                child.standardError = stderrPipe
                child.environment = environment
                child.currentDirectoryURL = currentDirectoryURL

                let shouldTerminate = self.storeProcess(child)
                if shouldTerminate {
                    self.finish(.failure(ProcessRunnerError.cancelled))
                    return
                }

                do {
                    try child.run()
                } catch {
                    self.finish(.failure(ProcessRunnerError.launchFailed(error.localizedDescription)))
                    return
                }

                // Cancellation can win the narrow window between storing the
                // Process and a successful run(). It cannot terminate a
                // pre-launch Process, so re-read the locked state immediately
                // after run() and apply the already-recorded error to the now
                // running child. The timeout is still installed exactly once
                // below and will be cancelled by finish().
                if let pendingError = self.pendingTerminationError() {
                    self.terminate(with: pendingError)
                }
                self.installTimeout(timeout)

                let stdoutBox = DataBox()
                let stderrBox = DataBox()
                let outputGroup = DispatchGroup()

                outputGroup.enter()
                self.queue.async {
                    stdoutBox.set(stdoutPipe.fileHandleForReading.readDataToEndOfFile())
                    outputGroup.leave()
                }

                outputGroup.enter()
                self.queue.async {
                    stderrBox.set(stderrPipe.fileHandleForReading.readDataToEndOfFile())
                    outputGroup.leave()
                }

                child.waitUntilExit()
                outputGroup.wait()

                let result = ProcessRunnerResult(
                    terminationStatus: child.terminationStatus,
                    stdoutData: stdoutBox.get(),
                    stderrData: stderrBox.get()
                )

                if result.terminationStatus == 0 {
                    self.finish(.success(result))
                } else {
                    self.finish(.failure(ProcessRunnerError.nonZeroExit(
                        status: result.terminationStatus,
                        stdout: result.stdout,
                        stderr: result.stderr
                    )))
                }
            }
        }

        func cancel() {
            terminate(with: .cancelled)
        }

        private func timeout() {
            let timeout = currentTimeout
            terminate(with: .timedOut(timeout))
        }

        private var currentTimeout: TimeInterval {
            stateLock.lock()
            defer { stateLock.unlock() }
            return timeoutValue
        }

        private var timeoutValue: TimeInterval = 0

        private func storeProcess(_ child: Process) -> Bool {
            stateLock.lock()
            process = child
            let shouldTerminate = cancellationRequested || completed
            stateLock.unlock()
            return shouldTerminate
        }

        private func pendingTerminationError() -> ProcessRunnerError? {
            stateLock.lock()
            defer { stateLock.unlock() }
            return terminalError ?? (cancellationRequested ? .cancelled : nil)
        }

        private func installTimeout(_ timeout: TimeInterval) {
            let workItem = DispatchWorkItem { [weak self] in
                self?.timeout()
            }

            stateLock.lock()
            timeoutValue = timeout
            timeoutWorkItem = workItem
            let alreadyCompleted = completed
            stateLock.unlock()

            guard !alreadyCompleted else { return }
            queue.asyncAfter(deadline: .now() + max(0, timeout), execute: workItem)
        }

        private func terminate(with error: ProcessRunnerError) {
            let child: Process?
            stateLock.lock()
            if completed {
                stateLock.unlock()
                return
            }
            cancellationRequested = true
            if terminalError == nil {
                terminalError = error
            }
            child = process
            stateLock.unlock()

            guard let child, child.isRunning else { return }
            child.terminate()

            let forceKillWorkItem = DispatchWorkItem { [weak self, weak child] in
                guard
                    let self,
                    let child,
                    self.shouldForceKill(child),
                    child.isRunning
                else { return }
                let pid = child.processIdentifier
                guard pid > 0 else { return }
                _ = Darwin.kill(pid, SIGKILL)
            }

            stateLock.lock()
            let shouldScheduleForceKill = self.forceKillWorkItem == nil && !completed
            if shouldScheduleForceKill {
                self.forceKillWorkItem = forceKillWorkItem
            }
            stateLock.unlock()

            if shouldScheduleForceKill {
                queue.asyncAfter(deadline: .now() + 0.75, execute: forceKillWorkItem)
            }
        }

        private func shouldForceKill(_ child: Process) -> Bool {
            stateLock.lock()
            defer { stateLock.unlock() }
            return !completed && terminalError != nil && process === child
        }

        private func finish(_ result: Result<ProcessRunnerResult, Error>) {
            let callback: ((Result<ProcessRunnerResult, Error>) -> Void)?
            let finalResult: Result<ProcessRunnerResult, Error>

            stateLock.lock()
            guard !completed else {
                stateLock.unlock()
                return
            }
            completed = true
            timeoutWorkItem?.cancel()
            forceKillWorkItem?.cancel()
            process = nil
            callback = completion
            completion = nil
            if let terminalError {
                finalResult = .failure(terminalError)
            } else {
                finalResult = result
            }
            stateLock.unlock()

            callback?(finalResult)
        }
    }
}
