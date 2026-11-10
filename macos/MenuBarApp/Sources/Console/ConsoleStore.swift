import Foundation
import SwiftUI

/// 主控制台的数据中枢：账号、额度、助手状态都从这里出。
///
/// 为什么集中在一处：
/// - 切换分页不能重新起助手进程，也不能重新打一遍 Magpie；
/// - 总览页和额度页必须看到同一份快照，否则两个页面数字会打架。
///
/// 这里只做"取数"，不做任何写入；扫码、签到等未接入能力不在这里假装。
@MainActor
final class ConsoleStore: ObservableObject {
    @Published private(set) var accounts: [ConsoleAccount] = []
    @Published private(set) var snapshot: QuotaSnapshot?
    @Published private(set) var accountsError: String?
    @Published private(set) var quotaError: String?
    @Published private(set) var loadingAccounts = false
    @Published private(set) var loadingQuota = false
    @Published private(set) var lastAccountsRefresh: Date?
    @Published private(set) var lastQuotaRefresh: Date?

    let worker: AccountWorkerManager
    private let settings: AppSettings

    init(worker: AccountWorkerManager, settings: AppSettings) {
        self.worker = worker
        self.settings = settings
    }

    var stats: ConsoleOverviewStats { ConsoleOverviewStats(accounts: accounts) }
    var quotaSummary: ConsoleQuotaSummary { ConsoleQuotaSummary(snapshot: snapshot) }

    /// 首次进入窗口时拉一遍。重复调用是安全的。
    func loadIfNeeded() async {
        if snapshot == nil && !loadingQuota {
            await refreshQuota(force: false)
        }
        if lastAccountsRefresh == nil && !loadingAccounts {
            await refreshAccounts()
        }
    }

    func refreshAll() async {
        async let accountsTask: Void = refreshAccounts()
        async let quotaTask: Void = refreshQuota(force: true)
        _ = await (accountsTask, quotaTask)
    }

    func refreshAccounts() async {
        guard !loadingAccounts else { return }
        loadingAccounts = true
        defer { loadingAccounts = false }

        // 助手没起就起一次；起不来时把原因原样带到界面上。
        if case .running = worker.state {} else {
            await worker.refresh()
        }
        guard case .running = worker.state else {
            accountsError = workerFailureText
            accounts = []
            lastAccountsRefresh = Date()
            return
        }

        do {
            let rows = try await worker.listAccounts()
            accounts = rows.compactMap(ConsoleAccount.init(json:))
            accountsError = nil
        } catch {
            accounts = []
            accountsError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
        lastAccountsRefresh = Date()
    }

    func refreshQuota(force: Bool = true) async {
        guard !loadingQuota else { return }
        loadingQuota = true
        defer { loadingQuota = false }

        let result = await WidgetStatusLoader.snapshot(force: force, port: settings.magpiePort)
        snapshot = result
        quotaError = result.accounts.isEmpty
            ? "没有读到额度。确认 Magpie 正在运行（端口 \(settings.magpiePort)）。"
            : nil
        lastQuotaRefresh = Date()
    }

    private var workerFailureText: String {
        switch worker.state {
        case .failed(let message):
            return message.components(separatedBy: "\n").first ?? "账号助手异常"
        case .starting:
            return "账号助手启动中，请稍后重试"
        case .stopped:
            return "账号助手未启动"
        case .running:
            return ""
        }
    }
}
