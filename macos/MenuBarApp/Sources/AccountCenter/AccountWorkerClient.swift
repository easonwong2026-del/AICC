import Foundation

/// 与内嵌助手 `aicc-account-worker` 的 stdio 客户端。
///
/// 设计要点：
/// - 一行一条 JSON，读写都在独立队列上，绝不阻塞主线程。
/// - 每个请求带自增 id，收到响应前保持 continuation；超时或进程退出必须把它唤醒，
///   否则调用方会永远挂住。
/// - 助手崩溃时所有在途请求立刻失败，由上层决定是否重启。
/// `@unchecked Sendable`：可变状态全部由 `lock` 保护，stdin 写入另由 `writeLock` 串行化。
final class AccountWorkerClient: @unchecked Sendable {
    private let executable: URL
    private let environment: [String: String]

    private let process = Process()
    private let stdinPipe = Pipe()
    private let stdoutPipe = Pipe()
    private let stderrPipe = Pipe()

    private let lock = NSLock()
    private let writeLock = NSLock()
    private var pending: [Int: CheckedContinuation<[String: Any], Error>] = [:]
    private var nextID = 1
    private var buffer = Data()
    private var closed = false
    private var stderrTail = ""

    init(executable: URL, environment: [String: String] = [:]) {
        self.executable = executable
        self.environment = environment
    }

    var isRunning: Bool { process.isRunning }

    /// 助手 stderr 的最后一段，用于设置页展示失败原因。
    var diagnostics: String {
        withLock { stderrTail }
    }

    func start() throws {
        process.executableURL = executable
        process.standardInput = stdinPipe
        process.standardOutput = stdoutPipe
        process.standardError = stderrPipe

        var env = ProcessInfo.processInfo.environment
        for (key, value) in environment { env[key] = value }
        process.environment = env

        stdoutPipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            guard let self else { return }
            if data.isEmpty {
                self.markClosed(reason: "助手关闭了输出管道")
            } else {
                self.ingest(data)
            }
        }

        stderrPipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            guard let self, !data.isEmpty else { return }
            let text = String(decoding: data, as: UTF8.self)
            self.withLock {
                self.stderrTail += text
                if self.stderrTail.count > 4096 {
                    self.stderrTail = String(self.stderrTail.suffix(4096))
                }
            }
        }

        process.terminationHandler = { [weak self] proc in
            self?.markClosed(reason: "助手进程退出（status \(proc.terminationStatus)）")
        }

        try process.run()
    }

    func call(
        _ method: String,
        params: [String: Any] = [:],
        timeout: TimeInterval = 10
    ) async throws -> [String: Any] {
        let id = withLock { () -> Int in
            nextID += 1
            return nextID
        }
        let payload = try AccountWorkerJSON.encodeRequest(id: id, method: method, params: params)

        return try await withCheckedThrowingContinuation { continuation in
            let alreadyClosed = withLock { () -> Bool in
                if closed { return true }
                pending[id] = continuation
                return false
            }
            if alreadyClosed {
                continuation.resume(throwing: AccountWorkerFailure.notRunning)
                return
            }

            do {
                try self.write(payload)
            } catch {
                fail(id: id, error: .transport(error.localizedDescription))
                return
            }

            DispatchQueue.global().asyncAfter(deadline: .now() + timeout) { [weak self] in
                self?.fail(id: id, error: .timeout)
            }
        }
    }

    func stop() {
        stdoutPipe.fileHandleForReading.readabilityHandler = nil
        stderrPipe.fileHandleForReading.readabilityHandler = nil

        if process.isRunning {
            process.terminate()
        }
        // 不在主线程 waitUntilExit：terminationHandler 会负责把在途请求唤醒。
        markClosed(reason: "助手已停止")
    }

    // MARK: - 内部

    private func ingest(_ data: Data) {
        var results: [(CheckedContinuation<[String: Any], Error>, Result<[String: Any], Error>)] = []
        withLock {
            buffer.append(data)
            while let line = AccountWorkerJSON.takeLine(from: &buffer) {
                guard let object = AccountWorkerJSON.decodeObject(line),
                      let id = AccountWorkerJSON.responseID(object),
                      let continuation = pending.removeValue(forKey: id)
                else {
                    // 通知（没有 id）或没有人等待的响应，直接丢弃。
                    continue
                }
                if let error = AccountWorkerJSON.rpcError(from: object) {
                    results.append((continuation, .failure(AccountWorkerFailure.remote(error))))
                } else {
                    results.append((continuation, .success(object["result"] as? [String: Any] ?? [:])))
                }
            }
        }
        // 唤醒必须放在锁外，否则会把上层回调拖进锁里。
        for (continuation, result) in results {
            continuation.resume(with: result)
        }
    }

    private func fail(id: Int, error: AccountWorkerFailure) {
        let continuation = withLock { pending.removeValue(forKey: id) }
        continuation?.resume(throwing: error)
    }

    private func markClosed(reason: String) {
        let waiting = withLock { () -> [CheckedContinuation<[String: Any], Error>] in
            guard !closed else { return [] }
            closed = true
            let all = Array(pending.values)
            pending.removeAll()
            return all
        }
        for continuation in waiting {
            continuation.resume(throwing: AccountWorkerFailure.transport(reason))
        }
    }

    /// 多条请求可能同时到达，FileHandle 并发写会交错，必须串行化。
    private func write(_ payload: Data) throws {
        writeLock.lock()
        defer { writeLock.unlock() }
        try stdinPipe.fileHandleForWriting.write(contentsOf: payload)
    }

    private func withLock<T>(_ body: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body()
    }
}
