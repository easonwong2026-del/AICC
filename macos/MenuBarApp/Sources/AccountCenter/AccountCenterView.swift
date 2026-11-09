import SwiftUI

// PR2a: WorkBuddy Switch 风格的只读界面预览。
// 真实账号仅从 PR1a 的 accounts.list 读取；扫码、签到和积分查询未接线。
private enum AccountCenterPage: String, CaseIterable, Identifiable {
    case accounts = "账号管理"
    case checkins = "签到任务"
    case credits = "积分统计"
    case settings = "设置"

    var id: String { rawValue }

    var icon: String {
        switch self {
        case .accounts: return "person.2"
        case .checkins: return "checkmark.calendar"
        case .credits: return "chart.bar"
        case .settings: return "gearshape"
        }
    }

    var detail: String {
        switch self {
        case .accounts: return "统一查看 WorkBuddy 和 Qoder CN 账号状态"
        case .checkins: return "每日签到、成长任务与领取记录"
        case .credits: return "积分余额、到期时间与领取趋势"
        case .settings: return "账号助手与本地数据状态"
        }
    }
}

private enum AccountCenterScope: String, CaseIterable, Identifiable {
    case all = "全部"
    case workbuddy = "WorkBuddy"
    case qoder = "Qoder CN"

    var id: String { rawValue }

    func includes(_ platform: String) -> Bool {
        switch self {
        case .all: return true
        case .workbuddy: return platform == "workbuddy"
        case .qoder: return platform == "qoder_cn"
        }
    }
}

private struct AccountCenterAccount: Identifiable {
    let id: String
    let platform: String
    let uid: String
    let nickname: String
    let authStatus: String
    let checkinEnabled: Bool

    var platformName: String { platform == "workbuddy" ? "WorkBuddy" : "Qoder CN" }

    var authLabel: String {
        switch authStatus {
        case "valid": return "认证有效"
        case "expired": return "已过期"
        case "invalid": return "认证失效"
        case "denied": return "授权被拒"
        default: return "待验证"
        }
    }

    init(
        id: String,
        platform: String,
        uid: String,
        nickname: String,
        authStatus: String = "unknown",
        checkinEnabled: Bool = false
    ) {
        self.id = id
        self.platform = platform
        self.uid = uid
        self.nickname = nickname
        self.authStatus = authStatus
        self.checkinEnabled = checkinEnabled
    }

    init?(json: [String: Any]) {
        guard let id = json["id"] as? String,
              let platform = json["platform"] as? String,
              let uid = json["uid"] as? String else { return nil }
        let name = json["nickname"] as? String ?? ""
        self.init(
            id: id,
            platform: platform,
            uid: uid,
            nickname: name.isEmpty ? "未命名账号" : name,
            authStatus: json["authStatus"] as? String ?? "unknown",
            checkinEnabled: json["checkinEnabled"] as? Bool ?? false
        )
    }

    // 只在标有「演示数据」的预览模式出现，从不写入真实数据库。
    static let examples: [AccountCenterAccount] = [
        .init(id: "demo-01", platform: "workbuddy", uid: "demo-wb-001", nickname: "工作账号 · 示例", authStatus: "valid", checkinEnabled: true),
        .init(id: "demo-02", platform: "workbuddy", uid: "demo-wb-002", nickname: "备用账号 · 示例", authStatus: "unknown"),
        .init(id: "demo-03", platform: "qoder_cn", uid: "demo-qd-003", nickname: "Qoder · 示例", authStatus: "valid"),
    ]
}

struct AccountCenterView: View {
    @ObservedObject var worker: AccountWorkerManager

    @State private var page: AccountCenterPage = .accounts
    @State private var scope: AccountCenterScope = .all
    @State private var preview = true
    @State private var accounts: [AccountCenterAccount] = []
    @State private var loading = false
    @State private var loadError: String?

    private let sidebar = Color(red: 0.105, green: 0.157, blue: 0.245)
    private let canvas = Color(red: 0.065, green: 0.091, blue: 0.16)
    private let panel = Color(red: 0.13, green: 0.18, blue: 0.27)
    private let accent = Color(red: 0.17, green: 0.81, blue: 0.67)

    private var visibleAccounts: [AccountCenterAccount] {
        (preview ? AccountCenterAccount.examples : accounts)
            .filter { scope.includes($0.platform) }
    }

    var body: some View {
        HStack(spacing: 0) {
            sidebarView
            Rectangle().fill(.white.opacity(0.07)).frame(width: 1)
            VStack(alignment: .leading, spacing: 0) {
                header
                Rectangle().fill(.white.opacity(0.07)).frame(height: 1)
                ScrollView {
                    Group {
                        switch page {
                        case .accounts: accountsPage
                        case .checkins: checkinsPage
                        case .credits: creditsPage
                        case .settings: settingsPage
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(26)
                }
            }
            .background(canvas)
        }
        .frame(width: 940, height: 610)
        .preferredColorScheme(.dark)
        .task { await loadAccounts() }
    }

    private var sidebarView: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 10) {
                Image(systemName: "person.crop.rectangle.stack.fill")
                    .font(.title2)
                    .foregroundStyle(accent)
                    .frame(width: 34, height: 34)
                    .background(accent.opacity(0.12), in: RoundedRectangle(cornerRadius: 9))
                VStack(alignment: .leading, spacing: 2) {
                    Text("AICC").font(.headline.weight(.bold))
                    Text("AI 账号与签到中心").font(.caption2).foregroundStyle(.secondary)
                }
            }
            .padding(.bottom, 18)

            ForEach(AccountCenterPage.allCases) { item in
                Button {
                    page = item
                } label: {
                    HStack(spacing: 11) {
                        Image(systemName: item.icon).frame(width: 18)
                        Text(item.rawValue)
                        Spacer(minLength: 0)
                    }
                    .font(.system(size: 13, weight: page == item ? .semibold : .medium))
                    .foregroundStyle(page == item ? .white : .white.opacity(0.66))
                    .padding(.horizontal, 12)
                    .frame(height: 38)
                    .background(
                        page == item ? Color.white.opacity(0.105) : .clear,
                        in: RoundedRectangle(cornerRadius: 9)
                    )
                }
                .buttonStyle(.plain)
            }

            Spacer()

            VStack(alignment: .leading, spacing: 7) {
                HStack(spacing: 7) {
                    Circle()
                        .fill(workerHealthy ? accent : .orange)
                        .frame(width: 7, height: 7)
                    Text(workerHealthy ? "账号助手正常" : "账号助手待检查")
                        .font(.caption)
                        .foregroundStyle(.white.opacity(0.8))
                }
                Text("PR2a · 界面预览")
                    .font(.caption2)
                    .foregroundStyle(.white.opacity(0.44))
            }
        }
        .padding(18)
        .frame(width: 204)
        .frame(maxHeight: .infinity, alignment: .topLeading)
        .background(sidebar)
    }

    private var workerHealthy: Bool {
        if case .running(let health) = worker.state { return health.isHealthy }
        return false
    }

    private var header: some View {
        HStack {
            VStack(alignment: .leading, spacing: 5) {
                Text(page.rawValue)
                    .font(.system(size: 23, weight: .semibold))
                Text(page.detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            if preview {
                Text("演示数据 · 非真实账号")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.orange)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(.orange.opacity(0.12), in: Capsule())
            }
        }
        .padding(.horizontal, 26)
        .padding(.vertical, 22)
    }

    private var accountsPage: some View {
        VStack(alignment: .leading, spacing: 19) {
            HStack(spacing: 9) {
                Picker("账号来源", selection: $scope) {
                    ForEach(AccountCenterScope.allCases) { option in
                        Text(option.rawValue).tag(option)
                    }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .frame(maxWidth: 320)

                Spacer()

                Button(preview ? "查看真实账号" : "预览示例") {
                    preview.toggle()
                }
                .buttonStyle(.bordered)
                Button {
                    Task { await loadAccounts() }
                } label: {
                    Label("刷新", systemImage: "arrow.clockwise")
                }
                .disabled(loading)
                .buttonStyle(.bordered)
                Button("扫码添加") {}
                    .buttonStyle(.borderedProminent)
                    .disabled(true)
                    .help("PR1b 真实账号登录验证后开放")
            }

            HStack {
                Text("账号 \(visibleAccounts.count)")
                    .font(.subheadline.weight(.semibold))
                Spacer()
                Text(preview ? "仅作界面展示，不影响任何真实账号" : "数据来自本机 AICC 账号数据库")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            if !preview, let loadError {
                Label(loadError, systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.orange)
                    .font(.caption)
            }

            if visibleAccounts.isEmpty {
                VStack(spacing: 13) {
                    Image(systemName: "person.crop.rectangle.stack")
                        .font(.system(size: 34))
                        .foregroundStyle(accent)
                    Text("暂无账号").font(.headline)
                    Text("真实扫码登录将在 PR1b 接入；现在可以先查看演示卡片。")
                        .foregroundStyle(.secondary)
                        .font(.caption)
                    Button("预览示例账号") { preview = true }
                        .buttonStyle(.borderedProminent)
                }
                .frame(maxWidth: .infinity)
                .frame(height: 250)
                .background(panel, in: RoundedRectangle(cornerRadius: 15))
            } else {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 270), spacing: 14)], spacing: 14) {
                    ForEach(visibleAccounts) { account in
                        AccountCenterCard(account: account, isDemo: preview)
                    }
                }
            }
        }
    }

    private var checkinsPage: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label("签到功能尚未接入", systemImage: "info.circle")
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .padding(.bottom, 7)
            taskRow("WorkBuddy 每日签到", detail: "查看签到状态、领取积分", symbol: "calendar.badge.checkmark")
            taskRow("成长中心任务", detail: "签到、盲盒与活动任务", symbol: "sparkles")
            taskRow("Buddy 旅行", detail: "派出、返回与奖励领取", symbol: "airplane")
            Text("本页不发送领取请求，也不会自动触发任务。PR1b 联调后再逐项启用。")
                .font(.caption)
                .foregroundStyle(.secondary)
                .padding(.top, 8)
        }
    }

    private func taskRow(_ title: String, detail: String, symbol: String) -> some View {
        HStack(spacing: 14) {
            Image(systemName: symbol)
                .font(.title3)
                .foregroundStyle(accent)
                .frame(width: 38, height: 38)
                .background(accent.opacity(0.10), in: RoundedRectangle(cornerRadius: 10))
            VStack(alignment: .leading, spacing: 4) {
                Text(title).font(.subheadline.weight(.semibold))
                Text(detail).font(.caption).foregroundStyle(.secondary)
            }
            Spacer()
            Text("待接入")
                .font(.caption)
                .foregroundStyle(.secondary)
            Button("立即执行") {}
                .disabled(true)
        }
        .padding(17)
        .background(panel, in: RoundedRectangle(cornerRadius: 13))
    }

    private var creditsPage: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(spacing: 14) {
                metric("积分余额", value: "—", symbol: "circle.dotted.circle")
                metric("即将到期", value: "—", symbol: "clock")
                metric("最近领取", value: "—", symbol: "gift")
            }
            VStack(spacing: 13) {
                Image(systemName: "chart.xyaxis.line")
                    .font(.system(size: 38))
                    .foregroundStyle(accent.opacity(0.65))
                Text("等待真实积分数据").font(.headline)
                Text("PR1a 尚不支持积分查询。这里不会展示虚构余额或虚构领取记录。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity)
            .frame(height: 220)
            .background(panel, in: RoundedRectangle(cornerRadius: 15))
        }
    }

    private func metric(_ title: String, value: String, symbol: String) -> some View {
        VStack(alignment: .leading, spacing: 11) {
            Label(title, systemImage: symbol)
                .font(.caption)
                .foregroundStyle(.secondary)
            Text(value)
                .font(.system(size: 27, weight: .semibold))
                .monospacedDigit()
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(17)
        .background(panel, in: RoundedRectangle(cornerRadius: 13))
    }

    private var settingsPage: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 12) {
                Text("账号助手").font(.headline)
                Text(workerStatus)
                    .font(.subheadline)
                    .foregroundStyle(workerHealthy ? accent : .secondary)
                Text(worker.helperPathDescription)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
                HStack {
                    Button("检查助手") { Task { await loadAccounts() } }
                    Button("测试钥匙串") { Task { await worker.checkSecureStore() } }
                }
                Text("凭据存储：\(worker.secureStoreSummary)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .padding(20)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(panel, in: RoundedRectangle(cornerRadius: 14))
            Text("当前界面只读。不会解密 WorkBuddy Electron 信封、修改客户端认证文件或启用后台签到。")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var workerStatus: String {
        switch worker.state {
        case .stopped: return "未启动"
        case .starting: return "启动中…"
        case .running(let health): return health.summary
        case .failed(let message): return message.components(separatedBy: "\n").first ?? "助手异常"
        }
    }

    @MainActor
    private func loadAccounts() async {
        guard !loading else { return }
        loading = true
        defer { loading = false }
        await worker.refresh()
        do {
            let rows = try await worker.listAccounts()
            accounts = rows.compactMap(AccountCenterAccount.init(json:))
            loadError = nil
        } catch {
            accounts = []
            loadError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }
}

private struct AccountCenterCard: View {
    let account: AccountCenterAccount
    let isDemo: Bool

    private let accent = Color(red: 0.17, green: 0.81, blue: 0.67)
    private let panel = Color(red: 0.13, green: 0.18, blue: 0.27)

    var body: some View {
        VStack(alignment: .leading, spacing: 15) {
            HStack(alignment: .top, spacing: 11) {
                Image(systemName: account.platform == "workbuddy" ? "bubble.left.and.bubble.right.fill" : "chevron.left.forwardslash.chevron.right")
                    .font(.title3)
                    .foregroundStyle(accent)
                    .frame(width: 36, height: 36)
                    .background(accent.opacity(0.11), in: RoundedRectangle(cornerRadius: 9))
                VStack(alignment: .leading, spacing: 5) {
                    Text(account.nickname)
                        .font(.subheadline.weight(.semibold))
                        .lineLimit(1)
                    Text(account.platformName)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer(minLength: 0)
                Text(isDemo ? "演示" : account.authLabel)
                    .font(.caption2)
                    .foregroundStyle(isDemo ? .orange : accent)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 5)
                    .background((isDemo ? Color.orange : accent).opacity(0.12), in: Capsule())
            }
            Text("UID · \(account.uid)")
                .font(.caption2.monospaced())
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .textSelection(.enabled)

            Rectangle().fill(.white.opacity(0.08)).frame(height: 1)

            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 5) {
                    Text("可用积分").font(.caption2).foregroundStyle(.secondary)
                    Text("—").font(.system(size: 26, weight: .semibold))
                }
                Spacer()
                VStack(alignment: .trailing, spacing: 5) {
                    Text("签到").font(.caption2).foregroundStyle(.secondary)
                    Text(account.checkinEnabled ? "已设置 · 待联调" : "未开启")
                        .font(.caption)
                }
            }
            HStack {
                Text("积分到期查询待接入")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                Spacer()
                Image(systemName: "ellipsis")
                    .foregroundStyle(.secondary)
            }
        }
        .padding(17)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(panel, in: RoundedRectangle(cornerRadius: 15))
        .overlay(
            RoundedRectangle(cornerRadius: 15)
                .stroke(.white.opacity(0.08), lineWidth: 1)
        )
    }
}
