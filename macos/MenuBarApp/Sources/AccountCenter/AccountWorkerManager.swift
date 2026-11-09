import Foundation
import SwiftUI

/// 管理内嵌助手进程：定位、启动、健康检查、失败回收。
///
/// PR1a 的范围只到"能起、能通、能自检"；真正的后台常驻与调度在 PR4，
/// 所以这里**不**接管 `ServerManager` / `AICCAppDelegate` 的生命周期。
@MainActor
final class AccountWorkerManager: ObservableObject {
    enum State: Equatable {
        case stopped
        case starting
        case running(AccountWorkerHealth)
        case failed(String)
    }

    @Published private(set) var state: State = .stopped
    @Published private(set) var lastChecked: Date?
    @Published private(set) var secureStoreSummary = "未检查"

    private var client: AccountWorkerClient?
    private var restartCount = 0

    /// SwiftUI 在 `@StateObject` 初始化时可能是非主线程上下文，所以开一个
    /// nonisolated 构造器，其余成员仍然只在主线程访问。
    nonisolated init() {}

    /// 账号中心数据目录，与 Rust 侧默认值保持一致。
    static var dataDirectory: URL {
        let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("Library/Application Support")
        return base
            .appendingPathComponent("AICC-Dashboard", isDirectory: true)
            .appendingPathComponent("accounts", isDirectory: true)
    }

    /// 助手在 `AICC.app/Contents/Helpers/` 下。
    /// 开发时可用 `AICC_ACCOUNT_WORKER_PATH` 指向 `target/release` 里的产物。
    static func locateHelper() -> URL? {
        if let override = ProcessInfo.processInfo.environment["AICC_ACCOUNT_WORKER_PATH"],
           !override.isEmpty {
            let url = URL(fileURLWithPath: override)
            if FileManager.default.isExecutableFile(atPath: url.path) { return url }
        }
        let url = Bundle.main.bundleURL.appendingPathComponent(AccountWorkerProtocol.helperRelativePath)
        return FileManager.default.isExecutableFile(atPath: url.path) ? url : nil
    }

    var helperPathDescription: String {
        Self.locateHelper()?.path ?? "未找到（App 未内嵌助手）"
    }

    /// 启动并按需做一次健康检查。重复调用是安全的。
    func refresh() async {
        if state == .starting { return }

        if client == nil || client?.isRunning != true {
            state = .starting
            guard startProcess() else { return }
        }

        guard let client else {
            state = .failed("助手未启动")
            return
        }

        do {
            let result = try await client.call(AccountWorkerProtocol.methodHealth, timeout: 8)
            guard let health = AccountWorkerHealth.parse(result) else {
                state = .failed("助手返回的 health 结构无法解析")
                return
            }
            guard health.isProtocolCompatible else {
                state = .failed("协议版本不匹配：助手 \(health.protocolVersion)，App \(AccountWorkerProtocol.version)")
                return
            }
            state = .running(health)
            lastChecked = Date()
            restartCount = 0
        } catch {
            let detail = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            let tail = client.diagnostics
            state = .failed(tail.isEmpty ? detail : "\(detail)\n\(tail)")
            // 进程已经不可用就丢掉，下一次 refresh 会重新拉起。
            if client.isRunning == false {
                client.stop()
                self.client = nil
            }
        }
    }

    /// 显式跑一次钥匙串读写探针。健康检查**故意**不做这件事，
    /// 否则每次启动 App 都会弹一次系统授权框。
    func checkSecureStore() async {
        guard let client, client.isRunning else {
            secureStoreSummary = "助手未运行"
            return
        }
        do {
            let result = try await client.call(AccountWorkerProtocol.methodSecureStatus, timeout: 20)
            let ok = result["ok"] as? Bool ?? false
            if ok {
                let backend = result["backend"] as? String ?? "?"
                let latency = (result["latencyMs"] as? NSNumber)?.intValue ?? 0
                secureStoreSummary = "可用（\(backend)，\(latency) ms）"
            } else {
                let kind = result["kind"] as? String ?? "error"
                let message = result["message"] as? String ?? "未知原因"
                secureStoreSummary = "不可用（\(kind)）：\(message)"
            }
        } catch {
            secureStoreSummary = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// PR2a 只读账号列表。真实数据由 Rust SQLite 返回，不修改账号或客户端凭据。
    func listAccounts() async throws -> [[String: Any]] {
        guard let client, client.isRunning else {
            throw AccountWorkerFailure.notRunning
        }
        let result = try await client.call("accounts.list", timeout: 10)
        guard let rows = result["accounts"] as? [[String: Any]] else {
            throw AccountWorkerFailure.malformedLine
        }
        return rows
    }

    func stop() {
        client?.stop()
        client = nil
        state = .stopped
    }

    // MARK: - 内部

    private func startProcess() -> Bool {
        guard let helper = Self.locateHelper() else {
            state = .failed("未找到 aicc-account-worker，App 可能是旧构建")
            return false
        }
        let dataDir = Self.dataDirectory
        let appVersion = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "unknown"
        let worker = AccountWorkerClient(
            executable: helper,
            environment: [
                "AICC_ACCOUNT_DATA_DIR": dataDir.path,
                "AICC_VERSION": appVersion,
                // 上游核心自己的存储也收进 AICC 目录，不碰用户的 ~/.wb-switch。
                "WB_SWITCH_HOME": dataDir.appendingPathComponent("wb-switch-home").path,
            ]
        )
        do {
            try worker.start()
            client = worker
            restartCount += 1
            return true
        } catch {
            state = .failed("助手启动失败：\(error.localizedDescription)")
            return false
        }
    }
}
