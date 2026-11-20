import Foundation
import SwiftUI

/// 管理内嵌助手进程：定位、启动、健康检查、失败回收。
///
/// PR1a 的范围只到"能起、能通、能自检"；真正的后台常驻与调度在 PR4，
/// 所以这里**不**接管 `ServerManager` / `AICCAppDelegate` 的生命周期。
@MainActor
final class AccountWorkerManager: ObservableObject {
    /// 整个 App 只有一个助手进程：主控制台和设置页共用这一个实例。
    /// 每个页面各建一个会让 `health` 探针重复起进程（PRD §七.2）。
    ///
    /// `nonisolated(unsafe)`：实例是常量，所有可变状态仍然只在主线程读写
    /// （方法全在 `@MainActor` 上），这里只是让 SwiftUI 的属性初始化器能拿到它。
    nonisolated(unsafe) static let shared = AccountWorkerManager()

    enum State: Equatable {
        case stopped
        case starting
        case running(AccountWorkerHealth)
        case failed(String)
    }

    @Published private(set) var state: State = .stopped
    @Published private(set) var lastChecked: Date?
    @Published private(set) var secureStoreSummary = "未检查"
    /// 全局授权优先级：`magpie`（默认）或 `own`。由助手持久化在 `app_settings`。
    @Published var preferredAuthSource = "magpie"
    /// 设置项读写的最近一次错误，界面上不静默。
    @Published private(set) var authSettingsError: String?
    /// 凭据存储现状：是否已同意明文 JSON，以及各来源各几条。
    /// 只有 `health` 会更新它 —— 那是助手自己的判断，界面不另算一份。
    @Published private(set) var credentialStorage: CredentialStoragePolicy?
    /// CRED-4：旧钥匙串凭据的盘点结果。`nil` = 还没查过。
    @Published private(set) var credentialMigration: CredentialMigrationState?
    /// 最近一次**核验**的结果。面板常态显示异常 / 待清理份数，而不是只留一句提示。
    @Published private(set) var credentialVerifyReport: CredentialVerifyReport?
    /// 最近一次迁移 / 核验 / 回滚的结论。界面直接显示这句，不自己组织措辞。
    @Published private(set) var credentialMigrationNotice: String?
    @Published private(set) var credentialMigrationBusy = false
    /// PR-B: the LAN bridge's TLS identity, as the worker reports it.
    @Published private(set) var bridgeTls: BridgeTlsStatus?
    @Published private(set) var bridgeTlsError: String?

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

    // MARK: - 局域网 TLS 身份（PR-B）

    /// Create or reuse the certificate the LAN bridge serves.
    ///
    /// Called before the bridge restarts with LAN access on: without a
    /// certificate the Python side refuses to open the TLS port (it never falls
    /// back to plaintext), so "LAN enabled but no listener" would be the result.
    @discardableResult
    func ensureBridgeTls(sans: [String] = []) async -> Bool {
        let params: [String: Any] = sans.isEmpty ? [:] : ["sans": sans]
        return await callBridgeTls(method: "tls.ensure", params: params)
    }

    /// Replace the root. Every paired device must confirm the new fingerprint.
    @discardableResult
    func resetBridgeTls() async -> Bool {
        await callBridgeTls(method: "tls.reset", params: [:])
    }

    private func callBridgeTls(method: String, params: [String: Any]) async -> Bool {
        if client == nil || client?.isRunning != true {
            guard startProcess() else {
                bridgeTlsError = "助手未启动，无法准备局域网证书"
                return false
            }
        }
        guard let client else {
            bridgeTlsError = "助手未启动，无法准备局域网证书"
            return false
        }
        do {
            let result = try await client.call(method, params: params, timeout: 30)
            let parsed = BridgeTlsStatus.parse(result)
            guard parsed.spki != nil else {
                bridgeTlsError = "助手没有返回指纹"
                return false
            }
            bridgeTls = parsed
            bridgeTlsError = nil
            return true
        } catch {
            bridgeTlsError = "证书准备失败：\(error.localizedDescription)"
            return false
        }
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
            credentialStorage = health.credentialStorage
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

    // MARK: - 全局授权优先级（PR #66 M3）

    /// 读一次首选来源。失败时保持当前值并记错误，不把界面卡在"未知"。
    func loadAuthPreferences() async {
        guard let client, client.isRunning else { return }
        do {
            let result = try await client.call(
                "settings.get",
                params: ["key": AccountWorkerProtocol.settingPreferredAuthSource],
                timeout: 8
            )
            if let value = result["value"] as? String,
               value == "magpie" || value == "own" {
                preferredAuthSource = value
            }
            authSettingsError = nil
        } catch {
            authSettingsError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// 写首选来源。助手侧会校验取值，非法值返回 `invalid_params`。
    func setPreferredAuthSource(_ value: String) async {
        guard value == "magpie" || value == "own" else { return }
        let previous = preferredAuthSource
        preferredAuthSource = value
        guard let client, client.isRunning else {
            authSettingsError = "助手未运行，设置没有保存"
            return
        }
        do {
            _ = try await client.call(
                "settings.set",
                params: [
                    "key": AccountWorkerProtocol.settingPreferredAuthSource,
                    "value": value,
                ],
                timeout: 8
            )
            authSettingsError = nil
        } catch {
            preferredAuthSource = previous
            authSettingsError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    // MARK: - 明文 JSON 的告知与开关（CRED-3-UI）

    /// 记录用户对"新建授权使用明文 JSON"的选择。
    ///
    /// 只写设置、不做迁移：助手侧对既有账号的存储方式没有任何影响
    /// （`write_own` 只对**还没有自有授权行**的账号生效）。
    /// 返回是否保存成功，失败原因进 `authSettingsError`。
    func setJSONCredentialStorage(_ enabled: Bool) async -> Bool {
        guard let client, client.isRunning else {
            authSettingsError = "助手未运行，设置没有保存"
            return false
        }
        do {
            _ = try await client.call(
                "settings.set",
                params: [
                    "key": AccountWorkerProtocol.settingCredentialsJSONConsent,
                    "value": enabled ? "1" : "0",
                ],
                timeout: 8
            )
            authSettingsError = nil
            // 重新拉一次 health：开关状态与各来源条数都由助手给出，界面不自己改本地值。
            await refresh()
            return true
        } catch {
            authSettingsError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            return false
        }
    }

    // MARK: - 旧 Keychain 凭据迁移（CRED-4）

    /// 刷新盘点。**只读**：不会读钥匙串里的令牌，只列引用与摘要。
    ///
    /// `keepNotice` 用于"操作之后顺带刷新"：那时用户要看的是刚拿到的结果，
    /// 不能被"没有需要迁移的凭据"这类状态文案盖掉。
    func refreshCredentialMigration(keepNotice: Bool = false) async {
        guard let client, client.isRunning else {
            if !keepNotice { credentialMigrationNotice = "助手未运行，无法读取凭据状态" }
            return
        }
        do {
            let result = try await client.call("credentials.migration.inventory", timeout: 10)
            credentialMigration = CredentialMigrationState.parse(result)
            if !keepNotice, let state = credentialMigration {
                credentialMigrationNotice = state.summary.needsAttention
                    ? nil
                    : "没有需要迁移的凭据"
            }
        } catch {
            if !keepNotice { credentialMigrationNotice = Self.describe(error) }
        }
    }

    /// 迁移全部可迁移的旧钥匙串凭据。
    ///
    /// 助手会先做备份（库快照 + 清单，**不含令牌**），再逐条搬；原始钥匙串条目
    /// 一个都不删，所以随时可以回滚。返回值不参与界面判断，结论看 notice。
    func migrateCredentials() async {
        await performMigrationAction(
            method: "credentials.migration.run",
            successPrefix: "已迁移"
        ) { result in
            let migrated = result["migrated"] as? Int ?? 0
            let skipped = result["skipped"] as? Int ?? 0
            let failed = result["failed"] as? Int ?? 0
            let backup = result["backup"] as? String
            var text = "已迁移 \(migrated) 条"
            if skipped > 0 { text += "，跳过 \(skipped) 条（期间已被重新授权）" }
            if failed > 0 { text += "，失败 \(failed) 条（可重新核验）" }
            if let backup { text += "。备份：\(backup)" }
            return text
        }
    }

    /// 回滚：把引用改回钥匙串，并删掉本次迁移产生的副本。
    func rollbackCredentialMigration() async {
        await performMigrationAction(
            method: "credentials.migration.rollback",
            successPrefix: "已回滚"
        ) { result in
            let done = result["rolledBack"] as? Int ?? 0
            let pendingCleanup = result["cleanupPending"] as? Int ?? 0
            let skipped = result["skipped"] as? Int ?? 0
            let failed = result["failed"] as? Int ?? 0
            var text = "已回滚 \(done) 条"
            if pendingCleanup > 0 {
                text += "，\(pendingCleanup) 份副本没删掉（下次回滚会重试）"
            }
            if skipped > 0 { text += "，跳过 \(skipped) 条（已被重新授权）" }
            if failed > 0 { text += "，失败 \(failed) 条" }
            return text
        }
    }

    /// 核验：逐条读回本地 JSON 并比对归属与指纹，同时扫描没有行引用的文件。
    func verifyCredentials() async {
        guard let client, client.isRunning else {
            credentialMigrationNotice = "助手未运行，无法核验"
            return
        }
        credentialMigrationBusy = true
        defer { credentialMigrationBusy = false }
        do {
            let result = try await client.call("credentials.migration.verify", timeout: 30)
            guard let report = CredentialVerifyReport.parse(result) else {
                credentialMigrationNotice = "核验返回的数据不完整"
                return
            }
            // 面板要能常态显示"异常几份"，不能只在点完按钮的那条文案里存在。
            credentialVerifyReport = report
            var text = "核验：\(report.ok) 条正常"
            if report.needsRelink > 0 { text += "，\(report.needsRelink) 条借用授权待重新关联" }
            if report.pendingKeychain > 0 { text += "，\(report.pendingKeychain) 条仍在钥匙串" }
            if report.broken > 0 { text += "，\(report.broken) 条异常" }
            if report.orphanCount > 0 { text += "，\(report.orphanCount) 个未引用文件" }
            if report.pendingCleanup > 0 {
                text += "，\(report.pendingCleanup) 份副本待清理（下次回滚会重试）"
            }
            if report.brokenFiles > 0 { text += "，\(report.brokenFiles) 个文件读不出来" }
            credentialMigrationNotice = text
        } catch {
            credentialMigrationNotice = Self.describe(error)
        }
    }

    /// CRED-5：把仍指向钥匙串的**借用授权**改成 Magpie 动态关联。
    ///
    /// 与迁移完全不同：一个令牌都不搬、不产生明文文件、不删旧钥匙串条目，
    /// 所以不需要明文同意。这一步做完，"读 Magpie 授权"才真的不再碰钥匙串。
    func relinkBorrowedCredentials() async {
        guard let client, client.isRunning else {
            credentialMigrationNotice = "助手未运行，重新关联没有执行"
            return
        }
        credentialMigrationBusy = true
        defer { credentialMigrationBusy = false }
        do {
            let result = try await client.call("credentials.migration.relink", timeout: 60)
            let relinked = result["relinked"] as? Int ?? 0
            let failed = result["failed"] as? Int ?? 0
            let pending = result["pending"] as? Int ?? 0
            var text = "已重新关联 \(relinked) 条"
            if pending > 0 { text += "，\(pending) 条还没关联上" }
            // 逐条结果里所有失败原因都要露出来，不只第一条。
            let reasons = Self.reasonSummary(result["items"] as? [[String: Any]] ?? [])
            if failed > 0 { text += "，写入失败 \(failed) 条" }
            if !reasons.isEmpty { text += "（\(reasons)）" }
            credentialMigrationNotice = text
            await refreshCredentialMigration(keepNotice: true)
        } catch {
            credentialMigrationNotice = Self.describe(error)
        }
    }

    /// 把 `items[]` 里 `ok == false` 的原因聚合成一句，按出现次数排序。
    nonisolated static func reasonSummary(_ items: [[String: Any]]) -> String {
        var counts: [String: Int] = [:]
        for item in items where item["ok"] as? Bool == false {
            let reason = item["reason"] as? String ?? "unknown"
            counts[reason, default: 0] += 1
        }
        return counts
            .sorted { $0.value == $1.value ? $0.key < $1.key : $0.value > $1.value }
            .map { "\($0.key) ×\($0.value)" }
            .joined(separator: "、")
    }

    /// 旧键搬运（G4）的显式重试。**只有**这里会让 `failed` 重新进自动流程。
    ///
    /// 会读一次钥匙串，可能弹系统授权框，所以必须由用户点。
    func retryKeychainKeyMigration() async {
        guard let client, client.isRunning else {
            credentialMigrationNotice = "助手未运行，重试没有执行"
            return
        }
        credentialMigrationBusy = true
        defer { credentialMigrationBusy = false }
        do {
            let result = try await client.call("credentials.keychain.retry", timeout: 60)
            let reset = result["reset"] as? Int ?? 0
            let moved = result["moved"] as? Int ?? 0
            let stillFailed = result["failedRemaining"] as? Int ?? 0
            var text = "已重试 \(reset) 条，搬运成功 \(moved) 条"
            if stillFailed > 0 { text += "，仍有 \(stillFailed) 条失败（可能还是被拒绝）" }
            credentialMigrationNotice = text
            await refreshCredentialMigration(keepNotice: true)
        } catch {
            credentialMigrationNotice = Self.describe(error)
        }
    }

    private func performMigrationAction(
        method: String,
        successPrefix: String,
        summarize: ([String: Any]) -> String
    ) async {
        guard let client, client.isRunning else {
            credentialMigrationNotice = "助手未运行，\(successPrefix)操作没有执行"
            return
        }
        credentialMigrationBusy = true
        defer { credentialMigrationBusy = false }
        do {
            let result = try await client.call(method, timeout: 60)
            credentialMigrationNotice = summarize(result)
            // 刷新只是更新计数，不能把刚拿到的结果盖掉。
            await refreshCredentialMigration(keepNotice: true)
        } catch {
            credentialMigrationNotice = Self.describe(error)
        }
    }

    private static func describe(_ error: Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
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

    // MARK: - WorkBuddy 扫码登录（PR1b-1）

    /// 发起一次登录（浏览器或扫码，同一个会话）。助手没起会先拉起来。
    ///
    /// `variant` 决定站点：国内版和国际版是两套 OAuth 与两套 API 域名，
    /// 不能靠"登录成功后再看 domain"来纠正。
    func startWorkbuddyLogin(variant: WorkbuddyVariant = .cn) async throws -> WorkbuddyLoginStart {
        if case .running = state {} else {
            await refresh()
        }
        guard case .running = state, let client, client.isRunning else {
            throw AccountWorkerFailure.notRunning
        }
        let result = try await client.call(
            "auth.workbuddy.start",
            params: ["variant": variant.rawValue],
            timeout: 20
        )
        guard let start = WorkbuddyLoginStart.parse(result) else {
            throw AccountWorkerFailure.malformedLine
        }
        return start
    }

    /// 轮询一次。返回 `pending` 时调用方应继续等。
    func pollWorkbuddyLogin(_ loginId: String) async throws -> WorkbuddyLoginPhase {
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call(
            "auth.workbuddy.poll",
            params: ["loginId": loginId],
            timeout: 20
        )
        return WorkbuddyLoginPhase.parse(result)
    }

    // MARK: - Qoder 设备授权（PR #73）

    /// 发起一次设备授权。发起阶段不联网，助手没起会先拉起来。
    func startQoderLogin(edition: QoderEdition) async throws -> QoderLoginStart {
        if case .running = state {} else {
            await refresh()
        }
        guard case .running = state, let client, client.isRunning else {
            throw AccountWorkerFailure.notRunning
        }
        let result = try await client.call(
            "qoder.auth.start",
            params: ["edition": edition.rawValue],
            timeout: 15
        )
        guard let start = QoderLoginStart.parse(result) else {
            throw AccountWorkerFailure.malformedLine
        }
        return start
    }

    /// 轮询一次。返回 `.pending` 时调用方应继续等。
    func pollQoderLogin(_ loginId: String) async throws -> QoderLoginPhase {
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call(
            "qoder.auth.poll",
            params: ["loginId": loginId],
            timeout: 20
        )
        return QoderLoginPhase.parse(result)
    }

    /// 取消是一次性通知，失败也不该打断界面。
    func cancelQoderLogin(_ loginId: String) async {
        guard let client, client.isRunning else { return }
        _ = try? await client.call(
            "qoder.auth.cancel",
            params: ["loginId": loginId],
            timeout: 5
        )
    }

    // MARK: - Qoder PAT 备用授权（PR #73）

    /// 保存 PAT。**令牌只走这一次 RPC**，助手直接写钥匙串，返回值里没有令牌。
    ///
    /// 失败不抛 `AccountWorkerFailure`：`rejected` / `forbidden` / `network` 都是
    /// 用户能自己处理的业务结论，界面要按类型给下一步，而不是"助手出错了"。
    func saveQoderPat(edition: QoderEdition, token: String) async throws -> Result<QoderPatResult, QoderPatFailure> {
        if case .running = state {} else {
            await refresh()
        }
        guard case .running = state, let client, client.isRunning else {
            throw AccountWorkerFailure.notRunning
        }
        let result = try await client.call(
            "qoder.auth.pat.save",
            params: ["edition": edition.rawValue, "token": token],
            timeout: 30
        )
        if let saved = QoderPatResult.parse(result) {
            return .success(saved)
        }
        return .failure(QoderPatFailure.parse(result))
    }

    /// 重新检测已存的 PAT 是否还有效（只打一次 `userinfo`）。
    func checkQoderPat(accountId: String) async throws -> Result<String, QoderPatFailure> {
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call(
            "qoder.auth.pat.check",
            params: ["accountId": accountId],
            timeout: 25
        )
        if result["state"] as? String == "valid" {
            let identity = result["identity"] as? [String: Any]
            let name = (identity?["name"] as? String) ?? (identity?["email"] as? String) ?? "有效"
            return .success(name)
        }
        return .failure(QoderPatFailure.parse(result))
    }

    /// 移除 PAT。返回值分成"删干净 / 被更新 / 没删干净"三种，
    /// 界面不能把后两种当成功（CRED-3b 复审 P0-A）。
    func removeQoderPat(accountId: String) async throws -> QoderPatRemoval {
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call(
            "qoder.auth.pat.remove",
            params: ["accountId": accountId],
            timeout: 15
        )
        return QoderPatRemoval.parse(result)
    }

    // MARK: - 统一任务汇总（PR #76）

    /// 只读本地记录的任务汇总。助手侧不会因此发任何请求。
    func tasksOverview() async throws -> TasksOverview {
        if case .running = state {} else {
            await refresh()
        }
        guard case .running = state, let client, client.isRunning else {
            throw AccountWorkerFailure.notRunning
        }
        let result = try await client.call("tasks.overview", params: [:], timeout: 20)
        guard let overview = TasksOverview(json: result) else {
            throw AccountWorkerFailure.malformedLine
        }
        return overview
    }

    // MARK: - Qoder 账号删除（PR #77）

    /// 删掉一个 Qoder **服务账号**：连同 AICC 钥匙串里的 browser access / refresh
    /// 与 PAT 一起清掉。**不碰** Magpie 的配置，也不删 Qoder 客户端的本机 Profile。
    ///
    /// 失败是业务结论，不是通信故障：助手会在钥匙串删不掉时保留账号与授权，
    /// 返回一条能直接给用户看的中文提示（含"可重试"），所以这里只抛
    /// `AccountWorkerFailure` 表示真的没能跟助手说上话。
    ///
    /// UI 入口由 V3 的账号详情页负责，本阶段只留调用面。
    func deleteQoderAccount(accountId: String) async throws -> QoderAccountDeletion {
        if case .running = state {} else {
            await refresh()
        }
        guard case .running = state, let client, client.isRunning else {
            throw AccountWorkerFailure.notRunning
        }
        let result = try await client.call(
            "qoder.accounts.delete",
            params: ["accountId": accountId],
            timeout: 30
        )
        guard let deletion = QoderAccountDeletion.parse(result) else {
            throw AccountWorkerFailure.malformedLine
        }
        return deletion
    }

    // MARK: - Qoder 每日 Credits（PR #75）

    /// 只读查询今天的活动。**不发写请求**。
    func qoderCreditsStatus(accountId: String) async throws -> QoderCreditsReport {
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call(
            "qoder.credits.status",
            params: ["accountId": accountId],
            timeout: 30
        )
        return QoderCreditsReport(json: result)
    }

    /// 手动领取。**这是有副作用的调用**：只有用户在界面上点了才发。
    func qoderCreditsClaim(accountId: String) async throws -> QoderCreditsReport {
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call(
            "qoder.credits.claim",
            params: ["accountId": accountId],
            timeout: 45
        )
        return QoderCreditsReport(json: result)
    }

    func qoderCreditsRuns(accountId: String, limit: Int = 10) async throws -> [QoderCreditRun] {
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call(
            "qoder.credits.runs",
            params: ["accountId": accountId, "limit": limit],
            timeout: 20
        )
        let rows = (result["runs"] as? [[String: Any]]) ?? []
        return rows.compactMap { QoderCreditRun(json: $0) }
    }

    func qoderCreditsAutoStatus() async throws -> [QoderCreditsAutoItem] {
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call(
            "qoder.credits.auto.status",
            params: [:],
            timeout: 20
        )
        let rows = (result["accounts"] as? [[String: Any]]) ?? []
        return rows.compactMap { QoderCreditsAutoItem(json: $0) }
    }

    /// 打开 / 关闭自动领取。助手侧对"未验证的版本"会直接拒绝，
    /// 所以这里的错误是业务结论，不是通信故障。
    func setQoderCreditsAuto(accountId: String, enabled: Bool) async throws {
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        _ = try await client.call(
            "qoder.credits.auto.setEnabled",
            params: ["accountId": accountId, "enabled": enabled],
            timeout: 20
        )
    }

    // MARK: - 从 Magpie 导入（PR1b-1）

    /// 读取 Magpie 里已登录的 WorkBuddy 账号（只读，不含令牌）。
    func magpiePreview() async throws -> MagpiePreview {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call("providers.magpie.accounts", timeout: 15)
        return MagpiePreview.parse(result)
    }

    /// 把选中的账号搬进 AICC：助手侧只复制 access token，不碰 refresh。
    func importMagpieAccounts(keys: [String]) async throws -> MagpieImportResult {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call(
            "auth.magpie.import",
            params: keys.isEmpty ? [:] : ["keys": keys],
            timeout: 30
        )
        return MagpieImportResult.parse(result)
    }

    /// 用库里的令牌核一次真实登录态。失败返回 `.invalid` 之类的状态，不抛。
    func workbuddyStatus(accountId: String) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call("auth.workbuddy.status", params: ["id": accountId], timeout: 20)
    }

    // MARK: - WorkBuddy 客户端账号切换（WB-SWITCH-1）

    /// 只读：客户端装没装、在不在跑、**当前登录的是谁**、每个账号能不能切。
    ///
    /// "当前登录"来自客户端自己的认证文件；这里不拿 AICC 的选中项替代它。
    func workbuddyClientStatus() async throws -> WorkbuddyClientStatus {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call("workbuddy.client.status", timeout: 20)
        return WorkbuddyClientStatus(json: result)
    }

    /// 切换会**关闭并重启 WorkBuddy 客户端**，最坏情况要等关进程 20 秒 + 校验 25 秒，
    /// 外加两次活跃查询，所以超时给到 180 秒。
    func workbuddyClientSwitch(
        accountId: String,
        observeActivity: Bool = true
    ) async throws -> WorkbuddySwitchOutcome {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call(
            "workbuddy.client.switch",
            params: ["accountId": accountId, "observeActivity": observeActivity],
            timeout: 180
        )
        return WorkbuddySwitchOutcome(json: result)
    }

    /// 把某一轮切换之前的登录态放回去。不传 `journalId` 时由助手挑（未完成优先）。
    ///
    /// `force = true` 只在用户**明确确认**"当前登录已经不是我切过去的那个账号，
    /// 仍然要覆盖它"时使用；不传时助手会拒绝并返回 `live_changed`。
    func workbuddyClientRestore(
        journalId: String? = nil,
        force: Bool = false
    ) async throws -> WorkbuddyRestoreOutcome {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        var params: [String: Any] = ["force": force]
        if let journalId { params["journalId"] = journalId }
        let result = try await client.call(
            "workbuddy.client.restore",
            params: params,
            timeout: 180
        )
        return WorkbuddyRestoreOutcome(json: result)
    }

    // MARK: - 夜间账号轮换（WB-ROTATE-1）

    /// 只读：计划、下一轮时间、当前这一轮、最近记录。
    func workbuddyRotationStatus() async throws -> WorkbuddyRotationStatus {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call("workbuddy.rotation.status", timeout: 20)
        return WorkbuddyRotationStatus(json: result)
    }

    /// 保存配置。`params` 只带要改的字段——后端会与现有配置合并。
    func workbuddyRotationSave(params: [String: Any]) async throws -> WorkbuddyRotationStatus {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call("workbuddy.rotation.save", params: params, timeout: 20)
        return WorkbuddyRotationStatus(json: result)
    }

    /// 开 / 关。打开时会先校验配置（缺账号会直接拒绝），不会悄悄打开一个跑不起来的计划。
    func workbuddyRotationSetEnabled(_ enabled: Bool) async throws -> WorkbuddyRotationStatus {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call(
            "workbuddy.rotation.setEnabled",
            params: ["enabled": enabled],
            timeout: 20
        )
        return WorkbuddyRotationStatus(json: result)
    }

    /// 预演：只演算会做什么、会跳过什么，不动客户端、不读令牌。
    func workbuddyRotationDryRun() async throws -> WorkbuddyRotationDryRun {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call("workbuddy.rotation.dryRun", timeout: 30)
        return WorkbuddyRotationDryRun(json: result)
    }

    /// 立即执行一轮（会关客户端，用于用户在场的短时验证）。占用当天名额。
    func workbuddyRotationRunNow() async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "workbuddy.rotation.runNow",
            params: ["confirm": true],
            timeout: 180
        )
    }

    /// 暂停并恢复原账号。
    func workbuddyRotationStop() async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call("workbuddy.rotation.stop", timeout: 180)
    }

    /// 手动恢复某一轮的最初账号。`force` 只用于用户明确确认"覆盖我现在的登录"。
    func workbuddyRotationRestoreOrigin(
        runId: String? = nil,
        force: Bool = false
    ) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        var params: [String: Any] = ["force": force]
        if let runId { params["runId"] = runId }
        return try await client.call("workbuddy.rotation.restoreOrigin", params: params, timeout: 180)
    }

    // MARK: - Buddy 旅行（PR1c）

    /// 本地快照，**不打接口**。界面轮询用这个，几秒一次也不会有网络开销。
    func travelSummary() async throws -> TravelSummary {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call("travel.summary", timeout: 10)
        return TravelSummary.parse(result)
    }

    /// 真的查一次服务端状态。`depart` / `claim` 会真的提交，只在用户点按钮时调。
    func travelSync(_ method: TravelMethod, accountId: String) async throws -> TravelSyncResult {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        // 一次同步最坏要串四五个 HTTP 请求（含 20 秒超时），给足余量。
        let result = try await client.call(method.rawValue, params: ["id": accountId], timeout: 90)
        return TravelSyncResult.parse(result)
    }

    func travelSetEnabled(accountId: String, enabled: Bool) async throws {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        _ = try await client.call(
            "travel.setEnabled",
            params: ["id": accountId, "enabled": enabled],
            timeout: 10
        )
    }

    /// 自动旅行的运行信息（承载进程、间隔、上次检查）。
    func travelAutoStatus() async throws -> TravelAutoStatus? {
        guard let client, client.isRunning else { return nil }
        let result = try await client.call("travel.auto.status", timeout: 8)
        return TravelAutoStatus(json: result)
    }

    // MARK: - 四平台额度（M4）

    /// 统一额度看板。`refresh` 时会真的打网络；否则优先用库里的快照。
    /// 四平台里 Qoder 两版返回 `unverified` 且零请求。助手侧有 20 秒硬预算 + 3 路并发，
    /// 这里给 45 秒是留余量：RPC 超时不应该先于助手的预算触发。
    func workerQuota(days: Int = 7, refresh: Bool) async throws -> [WorkerQuotaRow] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let result = try await client.call(
            "quota.board",
            params: ["days": days, "refresh": refresh],
            timeout: 45
        )
        let rows = (result["rows"] as? [[String: Any]]) ?? []
        return rows.compactMap { WorkerQuotaRow(json: $0) }
    }

    // MARK: - 成长中心（PR #67：只读）

    /// 扫一遍成长中心。**只有 GET**：任务列表、档案、能量、连签、兑换状态、
    /// 抽奖次数、Buddy 配额与旅行状态。
    ///
    /// 超时给 90 秒：账号之间是串行的，一个账号最坏要等 9 个接口各 15 秒。
    /// 传 `accountIds` 只扫指定账号，空数组表示全部。
    func growthScan(accountIds: [String] = []) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "growth.scan",
            params: accountIds.isEmpty ? [:] : ["accountIds": accountIds],
            timeout: 90
        )
    }

    // MARK: - 成长中心自动管理（PR #68）

    /// 开关状态与下次检查时间。只读。
    func growthAutoStatus() async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call("growth.auto.status", timeout: 10)
    }

    func setGrowthAutoEnabled(_ enabled: Bool) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "growth.auto.setEnabled",
            params: ["enabled": enabled],
            timeout: 15
        )
    }

    /// 跑一轮。`force` 只跳过间隔判断，开关关着时仍然是只读检查。
    /// 一轮可能要接单 + 领若干个奖，每个请求 15 秒上限，给足时间。
    func growthTick(force: Bool = false) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "growth.auto.tick",
            params: force ? ["force": true] : [:],
            timeout: 180
        )
    }

    /// 执行记录：每个任务最近一条 + 累计到账。
    func growthRuns(accountId: String, limit: Int = 50) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "growth.runs",
            params: ["accountId": accountId, "limit": limit],
            timeout: 15
        )
    }

    /// 成长活跃状态（PR #103）：每账号两个成长 GET（`/heatmap` + `/streak`），
    /// 外加本地当天的签到记录。
    ///
    /// 只读，但仍然会打服务端：一次四个账号 8 个请求、账号之间串行，给 60 秒。
    /// **不自动轮询** —— 由用户点「刷新」触发。
    func activityStatus(accountIds: [String] = []) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let params: [String: Any] = accountIds.isEmpty ? [:] : ["accountIds": accountIds]
        return try await client.call("growth.activity.status", params: params, timeout: 60)
    }

    /// 奖励状态（PR #98）：连登档位、抽奖次数、补签卡与可补签日期。
    ///
    /// 只读：worker 侧只有四个 GET。一次查询每账号 4 个请求、账号之间串行，
    /// 所以给 60 秒；也**不会**自动轮询 —— 由用户点「刷新」触发。
    func rewardStatus(accountIds: [String] = []) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        let params: [String: Any] = accountIds.isEmpty ? [:] : ["accountIds": accountIds]
        return try await client.call("growth.rewards.status", params: params, timeout: 60)
    }

    /// 正式写操作（PR #101）：抽奖 / 兑换 / 补签。
    ///
    /// `confirm: false` 只返回预览（账号、动作、余额、预计消耗），**不发请求**。
    /// 三个动作分三个方法，日志里能直接看出用户点了什么。
    func rewardDraw(accountId: String, confirm: Bool) async throws -> [String: Any] {
        try await rewardWrite("growth.rewards.draw", accountId: accountId, target: nil, confirm: confirm)
    }

    func rewardRedeem(accountId: String, tier: String, confirm: Bool) async throws -> [String: Any] {
        try await rewardWrite("growth.rewards.redeem", accountId: accountId, target: tier, confirm: confirm)
    }

    func rewardMakeup(accountId: String, date: String, confirm: Bool) async throws -> [String: Any] {
        try await rewardWrite("growth.rewards.makeup", accountId: accountId, target: date, confirm: confirm)
    }

    /// 只读预览：真的去打一次 GET，拿实时余额与资格。
    func rewardPreview(accountId: String, action: RewardAction, target: String?) async throws -> [String: Any] {
        var params: [String: Any] = ["accountId": accountId, "confirm": false]
        if let target { params["target"] = target }
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(method(for: action), params: params, timeout: 45)
    }

    /// 回查待核验记录。**只读服务端**，不重发。
    func rewardRecheck(accountId: String) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "growth.rewards.recheck",
            params: ["accountId": accountId],
            timeout: 45
        )
    }

    private func method(for action: RewardAction) -> String {
        switch action {
        case .draw: return "growth.rewards.draw"
        case .redeem: return "growth.rewards.redeem"
        case .makeup: return "growth.rewards.makeup"
        }
    }

    private func rewardWrite(
        _ method: String,
        accountId: String,
        target: String?,
        confirm: Bool
    ) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        var params: [String: Any] = ["accountId": accountId, "confirm": confirm]
        if let target { params["target"] = target }
        return try await client.call(method, params: params, timeout: 60)
    }

    // MARK: - 每日签到（PR #69）

    /// 查今日签到状态。默认会真的问一次服务端（只读 POST）；`cached` 时不联网。
    ///
    /// 超时 90 秒：账号之间串行，每个账号一次状态查询。
    func checkinStatus(accountIds: [String] = [], cached: Bool = false) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        var params: [String: Any] = [:]
        if !accountIds.isEmpty { params["accountIds"] = accountIds }
        if cached { params["cached"] = true }
        return try await client.call(
            "checkin.status",
            params: params,
            timeout: cached ? 15 : 90
        )
    }

    /// 用户显式点「立即签到」。不受自动开关约束，但仍然先查服务端。
    func checkinRun(accountId: String) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "checkin.run",
            params: ["accountId": accountId],
            timeout: 60
        )
    }

    /// 批量：只签服务端确认未签到的账号。
    func checkinRunPending(accountIds: [String] = []) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "checkin.runPending",
            params: accountIds.isEmpty ? [:] : ["accountIds": accountIds],
            timeout: 180
        )
    }

    /// 单账号自动签到开关。**不碰授权版本**（worker 里走专用写入）。
    func checkinSetEnabled(accountId: String, enabled: Bool) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "checkin.setEnabled",
            params: ["accountId": accountId, "enabled": enabled],
            timeout: 15
        )
    }

    func checkinAutoStatus() async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call("checkin.auto.status", timeout: 10)
    }

    /// 跑一轮自动签到。`force` 只跳过时间判断，不绕过账号开关。
    func checkinTick(force: Bool = false) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "checkin.auto.tick",
            params: force ? ["force": true] : [:],
            timeout: 180
        )
    }

    func checkinRecords(accountId: String? = nil, limit: Int = 30) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        var params: [String: Any] = ["limit": limit]
        if let accountId { params["accountId"] = accountId }
        return try await client.call("checkin.records", params: params, timeout: 15)
    }

    // MARK: - Qoder 双版本多账号（PR #70）

    /// 两个版本的安装情况。不碰钥匙串，可以随时调。
    func qoderEditionsDetect() async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call("qoder.editions.detect", timeout: 10)
    }

    func qoderProfilesList(edition: String? = nil) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        var params: [String: Any] = [:]
        if let edition { params["edition"] = edition }
        return try await client.call("qoder.profiles.list", params: params, timeout: 15)
    }

    /// Qoder CN / Global 的**平台服务账号**（不是本机快照）。
    ///
    /// 导入 Magpie 的 Qoder 账号后，账号只存在于这一步的返回值里：
    /// 本机有没有装客户端都不影响。
    func qoderAccountsList(edition: String? = nil) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        var params: [String: Any] = [:]
        if let edition { params["edition"] = edition }
        return try await client.call("qoder.accounts.list", params: params, timeout: 15)
    }

    /// 扫描本机已有的配置目录，登记成 Profile（不移动、不覆盖）。
    func qoderProfilesScan() async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call("qoder.profiles.scan", timeout: 30)
    }

    func qoderProfileCreate(edition: String, displayName: String) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "qoder.profiles.create",
            params: ["edition": edition, "displayName": displayName],
            timeout: 20
        )
    }

    /// 导入一份已有的账号快照目录（目录里必须有 `auth.v1.dat`）。
    func qoderProfileImport(
        edition: String,
        snapshotDir: String,
        displayName: String? = nil
    ) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        var params: [String: Any] = ["edition": edition, "snapshotDir": snapshotDir]
        if let displayName, !displayName.isEmpty { params["displayName"] = displayName }
        return try await client.call("qoder.profiles.import", params: params, timeout: 20)
    }

    /// 把客户端当前登录态存进该 Profile 的快照。
    func qoderProfileCapture(profileId: String) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "qoder.profiles.capture",
            params: ["profileId": profileId],
            timeout: 20
        )
    }

    /// 显式恢复未完成的切换。后端会先确认对应客户端已完全退出。
    func qoderProfileRecover() async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call("qoder.profiles.recover", params: [:], timeout: 30)
    }

    /// 只读检查认证。第一次会弹钥匙串授权，之后不再弹。
    func qoderProfileInspect(profileId: String) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "qoder.profiles.inspect",
            params: ["profileId": profileId],
            timeout: 30
        )
    }

    func qoderProfileRename(profileId: String, displayName: String) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "qoder.profiles.rename",
            params: ["profileId": profileId, "displayName": displayName],
            timeout: 15
        )
    }

    func qoderProfileOpen(profileId: String) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "qoder.profiles.open",
            params: ["profileId": profileId],
            timeout: 30
        )
    }

    func qoderProfileClose(profileId: String, force: Bool = false) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "qoder.profiles.close",
            params: ["profileId": profileId, "force": force],
            timeout: 30
        )
    }

    func qoderProfileActivate(profileId: String) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "qoder.profiles.activate",
            params: ["profileId": profileId],
            timeout: 40
        )
    }

    /// `purge = true` 才会删磁盘目录，且只对 AICC 托管的目录生效。
    func qoderProfileRemove(profileId: String, purge: Bool) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        return try await client.call(
            "qoder.profiles.remove",
            params: ["profileId": profileId, "purge": purge],
            timeout: 30
        )
    }

    func qoderProfilesStatus(edition: String? = nil) async throws -> [String: Any] {
        try await ensureRunning()
        guard let client, client.isRunning else { throw AccountWorkerFailure.notRunning }
        var params: [String: Any] = [:]
        if let edition { params["edition"] = edition }
        return try await client.call("qoder.profiles.status", params: params, timeout: 15)
    }

    private func ensureRunning() async throws {
        if case .running = state { return }
        await refresh()
        if case .running = state { return }
        // refresh() 期间可能正好在启动，给它一点时间落定。
        for _ in 0..<15 {
            if case .starting = state {
                try? await Task.sleep(nanoseconds: 200_000_000)
                continue
            }
            break
        }
        if case .running = state { return }
        throw AccountWorkerFailure.notRunning
    }

    /// 取消是一次性通知，失败也不该打断界面。
    func cancelWorkbuddyLogin(_ loginId: String) async {
        guard let client, client.isRunning else { return }
        _ = try? await client.call(
            "auth.workbuddy.cancel",
            params: ["loginId": loginId],
            timeout: 5
        )
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
