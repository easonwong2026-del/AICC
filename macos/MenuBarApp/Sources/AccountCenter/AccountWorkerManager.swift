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
