import AppKit
import SwiftUI

// MARK: - 离屏渲染样本
//
// 这是**视觉回归**用的样本数据，只存在于 `-D AICC_VISUAL_TEST` 的构建里。
// 它让截图能覆盖"长昵称 / 22 个账号 / 过期授权 / 失败任务"这类真实边界，
// 而不是拿空界面当验收证据。
//
// 数据本身不代表任何真实账号：UID、邮箱、额度都是编的。

#if AICC_CONSOLE_VISUAL

struct ConsoleVisualFixture {
    var accounts: [ConsoleAccount] = []
    var accountsError: String?
    var lastAccountsRefresh: Date?
    var snapshot: QuotaSnapshot?
    var quotaError: String?
    var workerQuotaRows: [WorkerQuotaRow] = []
    var workerQuotaError: String?
    var workerQuotaWindowDays = 7
    var tasksOverview: TasksOverview?
    var tasksOverviewError: String?
    var checkin: CheckinFleetStatus = .empty
    var checkinAuto: CheckinAutoStatus = .unknown
    var checkinError: String?
    var growthResult: GrowthScanResult = .empty
    var growthError: String?
    var lastGrowthScan: Date?
    var travelRows: [TravelAccountRow] = []
    var travelAuto: TravelAutoStatus?
    var travelError: String?
    var qoderProfiles: [QoderProfile] = []
    var qoderServiceAccounts: [QoderServiceAccount] = []
    var qoderEditions: [QoderEditionInfo] = []
    var qoderPendingSwitches: [QoderPendingSwitch] = []
    var qoderError: String?
    var qoderCreditsAuto: [QoderCreditsAutoItem] = []
    var qoderCreditsError: String?
}

extension ConsoleVisualFixture {
    /// 完整样本：4 个平台都有账号，含超长昵称、过期授权、失败任务。
    static var sample: ConsoleVisualFixture {
        var fixture = ConsoleVisualFixture()
        let now = Date()
        fixture.lastAccountsRefresh = now.addingTimeInterval(-320)
        fixture.lastGrowthScan = now.addingTimeInterval(-7200)

        fixture.accounts = [
            ConsoleAccount(json: [
                "id": "acc-wb-1",
                "platform": "workbuddy",
                "provider": "workbuddy",
                "uid": "wb_9f3c1d7a4b2e5f6089abcdef12345678",
                "nickname": "王翊 · 融影（上海）实业有限公司市场部日常运营主账号",
                "authStatus": "valid",
                "checkinEnabled": true,
                "updatedAt": "2026-11-14T14:02:11Z",
                "tokenExpiresAt": "2026-12-02T14:02:11Z",
                "authSource": "own",
                "authorizations": [
                    ["id": "auth-1", "kind": "own", "status": "valid",
                     "tokenExpiresAt": "2026-12-02T14:02:11Z", "keyMigrationState": "ready"],
                    ["id": "auth-2", "kind": "magpie", "status": "unknown"],
                ],
            ]),
            ConsoleAccount(json: [
                "id": "acc-wb-2",
                "platform": "workbuddy",
                "provider": "workbuddy",
                "uid": "wb_1122334455667788",
                "nickname": "设计外包",
                "authStatus": "expired",
                "checkinEnabled": false,
                "updatedAt": "2026-11-11T09:15:00Z",
                "tokenExpiresAt": "2026-11-12T09:15:00Z",
                "authSource": "magpie",
                "authorizations": [
                    ["id": "auth-3", "kind": "magpie", "status": "expired",
                     "tokenExpiresAt": "2026-11-12T09:15:00Z"],
                ],
            ]),
            ConsoleAccount(json: [
                "id": "acc-wb-ai-1",
                "platform": "workbuddy",
                "provider": "workbuddy_ai",
                "uid": "wbai_7788aabbccdd",
                "nickname": "International Demo",
                "authStatus": "valid",
                "updatedAt": "2026-11-13T22:40:00Z",
                "authSource": "own",
                "authorizations": [
                    ["id": "auth-4", "kind": "own", "status": "valid",
                     "tokenExpiresAt": "2026-12-13T22:40:00Z"],
                ],
            ]),
        ].compactMap { $0 }
        // 20 个额外账号：验证列表能独立滚动、筛选不卡。
        for index in 1...20 {
            if let account = ConsoleAccount(json: [
                "id": "acc-bulk-\(index)",
                "platform": "workbuddy",
                "provider": index % 5 == 0 ? "workbuddy_ai" : "workbuddy",
                "uid": "wb_bulk_\(String(format: "%04d", index))",
                "nickname": index % 4 == 0
                    ? "批量账号 \(index) · 名字很长用来测试截断与省略号的对齐表现"
                    : "批量账号 \(index)",
                "authStatus": index % 6 == 0 ? "invalid" : "valid",
                "updatedAt": "2026-11-10T08:00:00Z",
                "authSource": "own",
                "authorizations": [
                    ["id": "auth-bulk-\(index)", "kind": "own", "status": "valid"],
                ],
            ]) {
                fixture.accounts.append(account)
            }
        }

        fixture.qoderServiceAccounts = [
            QoderServiceAccount(json: [
                "id": "qsa-cn-1",
                "edition": "qoder_cn",
                "uid": "9f8e7d6c5b4a39281706f5e4d3c2b1a0",
                "nickname": "Qoder CN 主账号",
                "email": "ying.wang@example.com",
                "authorizations": [
                    ["source": "own_browser", "authStatus": "valid", "hasSecret": true,
                     "tokenExpiresAt": "2026-12-01T10:00:00Z"],
                    ["source": "own_pat", "authStatus": "valid", "hasSecret": true],
                    ["source": "magpie", "sourceKey": "magpie:qoder:1", "authStatus": "unknown",
                     "hasSecret": false],
                ],
            ]),
            QoderServiceAccount(json: [
                "id": "qsa-global-1",
                "edition": "qoder_global",
                "uid": "0a1b2c3d4e5f60718293a4b5c6d7e8f9",
                "nickname": "Qoder International Demo",
                "email": "demo@example.com",
                "authorizations": [
                    ["source": "magpie", "sourceKey": "magpie:qoder:2", "authStatus": "unknown",
                     "hasSecret": false],
                ],
            ]),
        ].compactMap { $0 }

        fixture.qoderProfiles = [
            QoderProfile(json: [
                "profileId": "qp-1",
                "edition": "qoder_cn",
                "displayName": "本机 · CN 客户端",
                "accountUid": "9f8e7d6c5b4a39281706f5e4d3c2b1a0",
                "email": "ying.wang@example.com",
                "liveDir": "/Users/example/Library/Application Support/Qoder/User",
                "snapshotDir": "/Users/example/Library/Application Support/AICC-Dashboard/qoder/qp-1",
                "authStatus": "valid",
                "expiresAt": "2026-12-01T10:00:00Z",
                "managed": true,
                "lastOpenedAt": "2026-11-14T09:12:00Z",
                "snapshotReady": true,
                "snapshotIncomplete": false,
                "active": true,
                "switchEnabled": false,
                "switchBlockedReason": "main.sqlite 里的账号数据未隔离，切换会串号",
                "runtime": ["state": "stopped"],
            ]),
            QoderProfile(json: [
                "profileId": "qp-2",
                "edition": "qoder_global",
                "displayName": "Intl 备用（未登录）",
                "liveDir": "/Users/example/Library/Application Support/Qoder Intl/User",
                "snapshotDir": "/Users/example/Library/Application Support/AICC-Dashboard/qoder/qp-2",
                "authStatus": "missing",
                "managed": true,
                "snapshotReady": false,
                "snapshotIncomplete": false,
                "active": false,
                "switchEnabled": false,
                "switchBlockedReason": "账号数据未隔离",
                "runtime": ["state": "stopped"],
            ]),
        ].compactMap { $0 }

        fixture.qoderEditions = [
            QoderEditionInfo(json: [
                "edition": "qoder_cn",
                "displayName": "Qoder CN",
                "bundleId": "com.qodercn.app",
                "installed": true,
                "clientVersion": "0.5.7",
                "dataDirs": ["a"],
                "parallelInstances": "不支持并行",
                "switchEnabled": false,
                "switchBlockedReason": "账号数据未隔离",
                "verifiedOnThisMachine": false,
                "loggedIn": true,
                "loggedInEmail": "ying.wang@example.com",
            ]),
            QoderEditionInfo(json: [
                "edition": "qoder_global",
                "displayName": "Qoder International",
                "bundleId": "com.qoder.app",
                "installed": true,
                "clientVersion": "0.5.7",
                "dataDirs": ["a"],
                "parallelInstances": "不支持并行",
                "switchEnabled": false,
                "switchBlockedReason": "账号数据未隔离",
                "verifiedOnThisMachine": true,
                "loggedIn": false,
            ]),
        ].compactMap { $0 }

        fixture.snapshot = ConsoleVisualFixture.magpieSnapshot

        fixture.workerQuotaRows = [
            WorkerQuotaRow(json: [
                "accountId": "acc-wb-1",
                "provider": "workbuddy",
                "nickname": "王翊 · 融影（上海）实业有限公司市场部日常运营主账号",
                "source": "own",
                "status": "ok",
                "unit": "credits",
                "total": 5000,
                "used": 403,
                "remaining": 4597.75,
                "plan": "标准版",
                "usage": ["windowDays": 7, "requests": 42, "credits": 403.2, "truncated": false],
                "capturedAt": "2026-11-14T14:02:11Z",
            ]),
            WorkerQuotaRow(json: [
                "accountId": "acc-wb-ai-1",
                "provider": "workbuddy_ai",
                "nickname": "International Demo",
                "source": "own",
                "status": "stale",
                "unit": "credits",
                "remaining": 880,
                "plan": "Team",
                "cached": true,
                "stale": true,
                "error": "本次读取超时，显示的是 11-14 09:20 的快照",
                "capturedAt": "2026-11-14T09:20:00Z",
            ]),
            WorkerQuotaRow(json: [
                "accountId": "qsa-cn-1",
                "provider": "qoder_cn",
                "nickname": "Qoder CN 主账号",
                "source": "own",
                "status": "ok",
                "credits": [
                    "state": "claimable",
                    "claimable": 1,
                    "claimed": 0,
                    "pending": 0,
                    "supported": true,
                    "today": ["credits": 100, "claimStatus": "claimable", "windowEnd": 1_763_000_000],
                ],
                "capturedAt": "2026-11-14T14:00:00Z",
            ]),
            WorkerQuotaRow(json: [
                "accountId": "qsa-global-1",
                "provider": "qoder_global",
                "nickname": "Qoder International Demo",
                "source": "magpie",
                "status": "unverified",
                "credits": ["state": "unsupported", "supported": false],
                "error": "该版本没有已证实的 Credits 接口，未发送任何请求",
            ]),
        ].compactMap { $0 }

        fixture.tasksOverview = TasksOverview(json: [
            "today": "2026-11-14",
            "accounts": ["total": 23],
            "checkin": ["done": 8, "pending": 3, "failed": 1],
            "growth": ["runs": 5, "failed": 1],
            "travel": ["claimed": 2, "pending": 1],
            "qoderCredits": ["claimed": 1, "failed": 0, "autoEnabled": 1, "claimable": 1],
            "needsReauth": 2,
            "autoEnabled": 6,
            "failureCount": 2,
            "recentFailures": [
                ["platform": "workbuddy", "accountId": "acc-wb-2", "nickname": "设计外包",
                 "task": "checkin", "status": "failed", "detail": "令牌已过期，需要重新登录",
                 "at": "2026-11-14T09:00:12Z"],
                ["platform": "workbuddy", "accountId": "acc-bulk-6", "nickname": "批量账号 6",
                 "task": "growth:accept", "status": "failed", "detail": "任务已下架",
                 "at": "2026-11-14T08:40:03Z"],
            ],
        ])

        fixture.checkin = CheckinFleetStatus(json: [
            "today": "2026-11-14",
            "cached": true,
            "counts": ["accounts": 12, "checked": 8, "enabled": 6, "unchecked": 3, "credit": 100],
            "accounts": [
                ["accountId": "acc-wb-1", "nickname": "王翊 · 融影（上海）实业有限公司市场部日常运营主账号",
                 "enabled": true, "state": "claimed", "credits": 100,
                 "localCheckedAt": "2026-11-14T09:00:31Z", "claimedByAiccToday": true,
                 "streakDays": 12, "dailyCredit": 100],
                ["accountId": "acc-wb-2", "nickname": "设计外包", "enabled": false,
                 "state": "failed", "errorKind": "unauthorized",
                 "errorDetail": "令牌已过期，需要重新登录"],
                ["accountId": "acc-bulk-1", "nickname": "批量账号 1", "enabled": true,
                 "state": "already", "credits": 50, "localCheckedAt": "2026-11-14T09:01:02Z"],
                ["accountId": "acc-bulk-2", "nickname": "批量账号 2", "enabled": true,
                 "state": "unchecked"],
                ["accountId": "acc-bulk-3", "nickname": "批量账号 3", "enabled": false,
                 "state": "unchecked"],
            ],
        ])
        fixture.checkinAuto = CheckinAutoStatus(json: [
            "enabled": true,
            "enabledAccounts": 6,
            "totalAccounts": 12,
            "planTime": "09:00",
            "timezone": "UTC+8（北京时间）",
            "lastTickAt": "2026-11-14T09:00:00Z",
            "nextRunAt": "2026-11-15T09:00:00Z",
            "running": false,
        ])

        fixture.growthResult = GrowthScanResult(json: [
            "scans": [
                ["accountId": "acc-wb-1",
                 "nickname": "王翊 · 融影（上海）实业有限公司市场部日常运营主账号",
                 "state": "ok",
                 "tasks": [
                    ["taskCode": "playbook_prompt", "title": "体验一次 Playbook",
                     "acceptStatus": "accepted", "progress": NSNull(),
                     "rewardCredit": 20, "rewardEnergy": 5],
                    ["taskCode": "Library_read", "title": "阅读资料库",
                     "acceptStatus": "pending", "progress": NSNull(),
                     "rewardCredit": 10, "rewardEnergy": 2],
                 ],
                 "counts": ["claimable": 1, "acceptable": 1, "completed": 2],
                 "scannedAt": "2026-11-14T13:00:00Z"],
                ["accountId": "acc-bulk-1", "nickname": "批量账号 1", "state": "ok",
                 "tasks": [], "counts": ["claimable": 0, "acceptable": 0, "completed": 0]],
            ],
            "overview": ["accounts": 2, "claimable": 1, "acceptable": 1],
        ])

        fixture.travelRows = [
            TravelAccountRow(json: [
                "accountId": "acc-wb-1",
                "nickname": "王翊 · 融影（上海）实业有限公司市场部日常运营主账号",
                "uid": "wb_9f3c1d7a",
                "travelEnabled": true,
                "authSource": "own",
                "travel": [
                    "label": "done_today", "locationName": "杭州", "claimed": true,
                    "rewardCredits": 30, "claimedAt": "2026-11-14T11:20:00Z",
                    "dailyLimitReached": true, "lastCheckedAt": "2026-11-14T11:20:00Z",
                ],
            ]),
            TravelAccountRow(json: [
                "accountId": "acc-bulk-1",
                "nickname": "批量账号 1",
                "uid": "wb_bulk_0001",
                "travelEnabled": false,
                "authSource": "own",
                "travel": ["label": "dispatchable", "locationName": "上海", "claimed": false],
            ]),
        ].compactMap { $0 }
        fixture.travelAuto = TravelAutoStatus(json: [
            "owner": "AICC 的账号助手进程",
            "runsOnlyWhileAppRunning": true,
            "departIntervalSecs": 1800,
            "claimIntervalSecs": 600,
            "enabledAccounts": 1,
            "busy": false,
            "lastTickAt": "2026-11-14T11:20:00Z",
        ])

        fixture.qoderCreditsAuto = [
            QoderCreditsAutoItem(json: [
                "accountId": "qsa-cn-1",
                "edition": "qoder_cn",
                "nickname": "Qoder CN 主账号",
                "autoEnabled": false,
                "supported": true,
                "lastRun": ["trigger": "manual", "outcome": "claimed", "credits": 100,
                            "executedAt": "2026-11-14T10:02:00Z"],
            ]),
            QoderCreditsAutoItem(json: [
                "accountId": "qsa-global-1",
                "edition": "qoder_global",
                "nickname": "Qoder International Demo",
                "autoEnabled": false,
                "supported": false,
            ]),
        ].compactMap { $0 }

        return fixture
    }

    /// 空状态：一个账号都没有。
    static var empty: ConsoleVisualFixture {
        ConsoleVisualFixture(lastAccountsRefresh: Date().addingTimeInterval(-60))
    }

    /// 失败状态：账号读取失败 + 额度读取失败。
    static var failure: ConsoleVisualFixture {
        var fixture = sample
        fixture.accounts = []
        fixture.qoderServiceAccounts = []
        fixture.qoderProfiles = []
        fixture.accountsError = "账号助手返回异常：worker 未就绪（connection refused）"
        fixture.qoderError = "qoder.editions.detect 超时"
        fixture.quotaError = "没有读到额度。确认 Magpie 正在运行（端口 5030）。"
        fixture.snapshot = nil
        fixture.workerQuotaError = "助手还没返回任何账号。"
        fixture.workerQuotaRows = []
        fixture.tasksOverview = nil
        fixture.tasksOverviewError = "tasks.overview 读取失败"
        return fixture
    }

    static var magpieSnapshot: QuotaSnapshot {
        let json = """
        {
          "schema_version": 3,
          "source": "Magpie",
          "available": true,
          "state": "Ready",
          "stale": false,
          "providers": ["codex", "claude"],
          "updated_at": "11-14 22:10",
          "refresh_seconds": 300,
          "fetched_at": 0,
          "accounts": [
            {
              "id": "codex-1", "provider": "codex", "provider_name": "Codex",
              "account_label": "main@example.com", "kind": "subscription", "plan": "Pro",
              "items": [
                {"name": "5 小时窗口", "used": 42.0, "remaining": 58.0, "remaining_text": "58%",
                 "display": null, "amount": null, "limit": null, "unit": null,
                 "unlimited": false, "reset": "11-15 03:00", "resets_at": null, "pool": null},
                {"name": "周窗口", "used": 12.0, "remaining": 88.0, "remaining_text": "88%",
                 "display": null, "amount": null, "limit": null, "unit": null,
                 "unlimited": false, "reset": "11-18 09:00", "resets_at": null, "pool": null}
              ],
              "balance": null, "balance_parts": [], "resets_count": 2, "renew": null,
              "last_served_at": null, "last": false, "failed": false, "deferred": false,
              "stale": false, "read_at": null
            },
            {
              "id": "claude-1", "provider": "claude", "provider_name": "Claude",
              "account_label": "ying.wang@example.com", "kind": "subscription", "plan": "Max",
              "items": [
                {"name": "5 小时窗口", "used": 71.0, "remaining": 29.0, "remaining_text": "29%",
                 "display": null, "amount": null, "limit": null, "unit": null,
                 "unlimited": false, "reset": "11-15 01:20", "resets_at": null, "pool": null}
              ],
              "balance": null, "balance_parts": [], "resets_count": 1, "renew": null,
              "last_served_at": null, "last": false, "failed": false, "deferred": false,
              "stale": true, "read_at": null
            },
            {
              "id": "deepseek-1", "provider": "deepseek", "provider_name": "DeepSeek",
              "account_label": "余额账户", "kind": "balance", "plan": null,
              "items": [], "balance": "¥ 128.40",
              "balance_parts": [{"label": "赠送", "text": "¥ 20.00", "percent": null}],
              "resets_count": null, "renew": null, "last_served_at": null, "last": false,
              "failed": false, "deferred": false, "stale": false, "read_at": null
            }
          ]
        }
        """
        let decoder = JSONDecoder()
        guard let data = json.data(using: .utf8),
              let snapshot = try? decoder.decode(QuotaSnapshot.self, from: data)
        else {
            return QuotaSnapshot()
        }
        return snapshot
    }
}

// MARK: - 渲染

@main
struct ConsoleVisualSmokeMain {
    @MainActor
    static func main() throws {
        let outputDir = "/private/tmp/aicc-console-qa"
        try FileManager.default.createDirectory(atPath: outputDir, withIntermediateDirectories: true)
        let application = NSApplication.shared
        application.setActivationPolicy(.prohibited)

        let settings = AppSettings.shared
        let store = ConsoleStore(worker: AccountWorkerManager.shared, settings: settings)
        store.applyVisualFixture(.sample)
        store.applyVisualFixture(.sample)  // 幂等：两次调用不应改变任何数据

        var count = 0

        // 四页 × 三尺寸 × 两种外观 = 24 张
        let sizes: [(CGFloat, CGFloat)] = [(960, 640), (1120, 760), (1440, 900)]
        for (width, height) in sizes {
            for scheme in [ColorScheme.light, .dark] {
                for page in ConsolePage.allCases {
                    let view = consoleView(store: store, settings: settings, scheme: scheme, page: page)
                        .frame(width: width, height: height)
                    let name = "console-\(page.rawValue)-\(Int(width))x\(Int(height))-\(schemeName(scheme))"
                    try render(view, size: CGSize(width: width, height: height), scheme: scheme,
                               to: "\(outputDir)/\(name).png")
                    count += 1
                }
            }
        }

        // 关键状态与弹层
        // 用闭包延迟构造：视图必须在渲染前一刻创建，否则 QA 钩子读到的会是最后一组参数。
        let extras: [(String, () -> AnyView, CGSize, ColorScheme)] = [
            ("accounts-qoder-detail-1120x760-light",
             { AnyView(consoleView(store: store, settings: settings, scheme: .light,
                                   page: .accounts, account: "qoderService:qsa-cn-1")) },
             CGSize(width: 1120, height: 760), .light),
            ("accounts-profile-detail-1120x760-dark",
             { AnyView(consoleView(store: store, settings: settings, scheme: .dark,
                                   page: .accounts, account: "qoderProfile:qp-1")) },
             CGSize(width: 1120, height: 760), .dark),
            ("quota-accounts-1120x760-light",
             { AnyView(consoleView(store: store, settings: settings, scheme: .light,
                                   page: .quota, quotaSource: "accounts")) },
             CGSize(width: 1120, height: 760), .light),
            ("checkin-credits-1120x760-light",
             { AnyView(consoleView(store: store, settings: settings, scheme: .light,
                                   page: .checkin, checkinPlatform: "qoder")) },
             CGSize(width: 1120, height: 760), .light),
            ("checkin-growth-1120x760-dark",
             { AnyView(consoleView(store: store, settings: settings, scheme: .dark,
                                   page: .checkin, workbuddyTask: "growth")) },
             CGSize(width: 1120, height: 760), .dark),
            ("add-account-platform-960x640-light",
             { AnyView(sheet(ConsoleAddAccountSheet(store: store, request: ConsoleAddRequest()) {})) },
             CGSize(width: 480, height: 420), .light),
            ("add-account-method-qoder-1120x760-light",
             { AnyView(sheet(ConsoleAddAccountSheet(
                store: store, request: ConsoleAddRequest(platform: .qoderCN)) {})) },
             CGSize(width: 480, height: 380), .light),
            ("qoder-pat-1120x760-light",
             { AnyView(sheet(QoderPatSheet(store: store, initialEdition: .cn) {})) },
             CGSize(width: 440, height: 470), .light),
            ("qoder-delete-confirm-1120x760-light",
             { AnyView(sheet(ConsoleDeleteAccountSheet(store: store, accountId: "qsa-cn-1") {})) },
             CGSize(width: 460, height: 340), .light),
        ]
        for (name, makeView, size, scheme) in extras {
            try render(makeView(), size: size, scheme: scheme, to: "\(outputDir)/\(name).png")
            count += 1
        }

        // 空状态与错误状态
        for (fixtureName, fixture) in [("empty", ConsoleVisualFixture.empty),
                                       ("error", ConsoleVisualFixture.failure)] {
            let stateStore = ConsoleStore(worker: AccountWorkerManager.shared, settings: settings)
            stateStore.applyVisualFixture(fixture)
            for scheme in [ColorScheme.light, .dark] {
                for page in [ConsolePage.overview, .accounts, .checkin] {
                    UserDefaults.standard.set(page.rawValue, forKey: "AICCConsolePage")
                    let view = MainConsoleView(store: stateStore, openSettings: {})
                        .environmentObject(settings)
                        .environmentObject(ServerManager.shared)
                        .environmentObject(LaunchAtLoginService.shared)
                        .preferredColorScheme(scheme)
                        .frame(width: 1120, height: 760)
                    let name = "state-\(fixtureName)-\(page.rawValue)-\(schemeName(scheme))"
                    try render(view, size: CGSize(width: 1120, height: 760), scheme: scheme,
                               to: "\(outputDir)/\(name).png")
                    count += 1
                }
            }
        }

        print("console visual QA: \(count) images -> \(outputDir)")
    }

    private static func schemeName(_ scheme: ColorScheme) -> String {
        scheme == .dark ? "dark" : "light"
    }

    @MainActor private static func consoleView(
        store: ConsoleStore,
        settings: AppSettings,
        scheme: ColorScheme,
        page: ConsolePage? = nil,
        account: String? = nil,
        quotaSource: String? = nil,
        checkinPlatform: String? = nil,
        workbuddyTask: String? = nil
    ) -> some View {
        UserDefaults.standard.set(page?.rawValue, forKey: "AICCConsolePage")
        UserDefaults.standard.set(account, forKey: "AICCConsoleAccount")
        UserDefaults.standard.set(quotaSource, forKey: "AICCConsoleQuotaSource")
        UserDefaults.standard.set(checkinPlatform, forKey: "AICCConsoleCheckinPlatform")
        UserDefaults.standard.set(workbuddyTask, forKey: "AICCConsoleWorkbuddyTask")
        return MainConsoleView(store: store, openSettings: {})
            .environmentObject(settings)
            .environmentObject(ServerManager.shared)
            .environmentObject(LaunchAtLoginService.shared)
            .preferredColorScheme(scheme)
    }

    /// 弹层外面垫一层画布，模拟 sheet 在窗口里的实际观感。
    private static func sheet<V: View>(_ content: V) -> some View {
        content
            .padding(16)
            .background(ConsoleTheme.canvas)
    }

    @MainActor private static func render<V: View>(
        _ view: V,
        size: CGSize,
        scheme: ColorScheme,
        to path: String
    ) throws {
        let hosting = NSHostingView(rootView: view.environment(\.colorScheme, scheme))
        hosting.appearance = NSAppearance(named: scheme == .dark ? .darkAqua : .aqua)
        hosting.frame = NSRect(origin: .zero, size: size)
        hosting.layoutSubtreeIfNeeded()
        // 让 SwiftUI 把布局跑完（首帧异步内容才会落位）。
        hosting.displayIfNeeded()
        RunLoop.current.run(until: Date().addingTimeInterval(0.35))
        hosting.layoutSubtreeIfNeeded()

        guard let bitmap = hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds) else {
            throw NSError(domain: "ConsoleVisual", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "bitmap failed: \(path)"])
        }
        hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
        guard let data = bitmap.representation(using: .png, properties: [:]) else {
            throw NSError(domain: "ConsoleVisual", code: 2,
                          userInfo: [NSLocalizedDescriptionKey: "png failed: \(path)"])
        }
        try data.write(to: URL(fileURLWithPath: path))
    }
}

#endif
