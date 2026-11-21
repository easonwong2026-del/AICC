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
    /// 奖励状态（PR #98）：账号详情里的「奖励」区块靠它渲染。
    var rewardStates: [String: RewardAccountState] = [:]
    var rewardFailures: [String: String] = [:]
    /// 成长活跃（PR #103）：总览页的「成长活跃」区块靠它渲染。
    var activity: ActivityFleetState = .empty
    var activityError: String?
    /// 离屏加载态：只显示进度，不伪造空数据。
    var isLoading = false
    /// WorkBuddy 客户端登录（WB-SWITCH-1）：账号页左栏顶部那行"当前登录"靠它渲染。
    var wbClientStatus: WorkbuddyClientStatus?
    var wbSwitchNotice: String?
    var wbSwitchFailed = false
    /// 夜间账号轮换（WB-ROTATE-1）：左栏的第二块卡片靠它渲染。
    var wbRotationStatus: WorkbuddyRotationStatus?
}

extension ConsoleVisualFixture {
    /// 相对 `now` 的 RFC3339，给"临时登录到期时间"这类样本用。
    static func iso(_ now: Date, _ seconds: TimeInterval) -> String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.string(from: now.addingTimeInterval(seconds))
    }

    /// 夜间轮换的一份样本。`state` 决定"正常 / 需要确认"，其余字段固定，
    /// 这样正常态与恢复异常态用的是同一套结构，截图可比。
    static func rotationStatusJSON(
        now: Date,
        state: String,
        detail: String?,
        restoreResult: String?
    ) -> [String: Any] {
        var current: [String: Any] = [
            "id": "run-demo",
            "planDate": "2026-11-20",
            "trigger": "scheduled",
            "state": state,
            "cursor": 1,
            "originUid": "wb_9f3c1d7a4b2e5f6089abcdef12345678",
            "originAccountId": "acc-wb-1",
            "originSnapshotTracked": true,
            "lastTargetUid": "wb_1122334455667788",
            "lastTargetAccountId": "acc-wb-2",
            "currentAccountId": "acc-mag-1",
            "currentUid": "wb_mag_0001",
            "nextSwitchAt": ConsoleVisualFixture.iso(now, 2400),
            "steps": [
                ["id": "st-1", "seq": 0, "accountId": "acc-wb-2", "uid": "wb_1122334455667788",
                 "mode": "own_refresh", "status": "done", "authPrecheck": "accepted",
                 "activityBefore": ["state": "ok", "todayActive": false, "streakDays": 3],
                 "activityAfter": ["state": "ok", "todayActive": true, "streakDays": 4]],
                ["id": "st-2", "seq": 1, "accountId": "acc-mag-1", "uid": "wb_mag_0001",
                 "mode": "magpie_access_temporary", "status": state == "recovery_required" ? "dwelling" : "dwelling"],
            ],
            "createdAt": "2026-11-20T14:30:00Z",
            "updatedAt": "2026-11-20T15:05:00Z",
        ]
        if let detail { current["detail"] = detail }
        if let restoreResult { current["restoreResult"] = restoreResult }

        var history: [[String: Any]] = [
            ["id": "run-yesterday", "planDate": "2026-11-19", "trigger": "scheduled",
             "state": "completed", "cursor": 1, "originSnapshotTracked": true,
             "restoreResult": "restored",
             "steps": [
                ["id": "h-1", "seq": 0, "accountId": "acc-wb-2", "uid": "wb_1122334455667788",
                 "status": "done", "mode": "own_refresh",
                 "activityBefore": ["state": "ok", "todayActive": false, "streakDays": 3],
                 "activityAfter": ["state": "ok", "todayActive": true, "streakDays": 4]],
                ["id": "h-2", "seq": 1, "accountId": "acc-mag-1", "uid": "wb_mag_0001",
                 "status": "skipped", "skipReason": "magpie_too_short_for_dwell"],
             ]],
            ["id": "run-manual", "planDate": "2026-11-18", "trigger": "manual",
             "state": "completed", "cursor": 0, "originSnapshotTracked": true,
             "restoreResult": "restored",
             "steps": [
                ["id": "m-1", "seq": 0, "accountId": "acc-bulk-1", "uid": "wb_bulk_0001",
                 "status": "done", "mode": "own_refresh",
                 "activityBefore": ["state": "ok", "todayActive": false, "streakDays": 0],
                 "activityAfter": ["state": "failed", "error": "超时"]],
             ]],
        ]
        // 真实后端把"当前这一轮"也算在最近记录里，样本保持一致。
        history.insert(
            ["id": "run-demo", "planDate": "2026-11-20", "trigger": "scheduled",
             "state": state, "cursor": 1, "originSnapshotTracked": true,
             "restoreResult": restoreResult ?? (state == "dwelling" ? "等待恢复" : "restore_failed"),
             "detail": detail ?? (state == "dwelling" ? "停留中，尚未到恢复时间" : ""),
             "steps": current["steps"] as Any],
            at: 0
        )

        var payload: [String: Any] = [
            "plan": [
                "enabled": true,
                "timezone": "Asia/Shanghai",
                "weekdays": [1, 2, 3, 4, 5, 6, 7],
                "startMinutes": 22 * 60 + 30,
                "endMinutes": 7 * 60,
                "accountIds": ["acc-wb-1", "acc-mag-1"],
                "dwellSeconds": 3600,
                "startText": "22:30",
                "endText": "07:00",
            ],
            "schedule": [
                "inWindow": true,
                "windowStart": ConsoleVisualFixture.iso(now, -1800),
                "windowEnd": ConsoleVisualFixture.iso(now, 28800),
            ],
            "live": ["uid": "wb_1122334455667788", "loggedIn": true],
            "client": ["installed": true, "running": true],
            "current": current,
            "lastRun": history.first ?? [:],
            "history": history,
            "scheduler": [
                "owner": "AICC.app 的账号助手进程",
                "runsWhileAppClosed": false,
                "tickSeconds": 60,
            ],
            "limits": [
                "lateStartToleranceSeconds": 600,
                "restoreReserveSeconds": 300,
                "userActiveIdleSeconds": 120,
                "supportedTimezones": ["Asia/Shanghai", "UTC"],
            ],
        ]
        // 真实后端把 `recovery_required` 排除在"活动轮次"之外：恢复失败后
        // `current` 是 nil，只有 `lastRun` 停在待处理。样本必须一样，
        // 否则截出来的界面比线上好看。
        if state == "recovery_required" {
            payload.removeValue(forKey: "current")
            payload["lastRun"] = current
        }
        return payload
    }

    /// 恢复失败的样本：用来渲染"需要你确认"的界面。
    static var rotationRecovery: ConsoleVisualFixture {
        var fixture = sample
        let now = Date()
        fixture.wbRotationStatus = WorkbuddyRotationStatus(
            json: rotationStatusJSON(
                now: now,
                state: "recovery_required",
                detail: "已写回快照，但客户端当前登录是 wb_1122…；请手动确认",
                restoreResult: "restore_failed"
            )
        )
        return fixture
    }

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
        // 只有一个 Magpie 关联的账号：临时切换的"目标"样本（授权来源与模式才自洽）。
        if let magpieOnly = ConsoleAccount(json: [
            "id": "acc-mag-1",
            "platform": "workbuddy",
            "provider": "workbuddy",
            "uid": "wb_mag_0001",
            "nickname": "Magpie 导入账号",
            "authStatus": "valid",
            "updatedAt": "2026-11-14T12:00:00Z",
            "authSource": "magpie",
            "authorizations": [
                ["id": "auth-mag-1", "kind": "magpie", "status": "valid",
                 "tokenExpiresAt": "2026-11-14T15:00:00Z"],
            ],
        ]) {
            fixture.accounts.append(magpieOnly)
        }

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
            // 长名称 + 超长数字：960 宽下也不允许出现省略号或重叠。
            WorkerQuotaRow(json: [
                "accountId": "acc-wb-long",
                "provider": "workbuddy",
                "nickname": "王翊 · 融影（上海）实业有限公司市场部日常运营主账号（超长名称）",
                "source": "own",
                "status": "ok",
                "unit": "credits",
                "total": 900_000_000,
                "used": 99_999_999,
                "remaining": 123_456_789.5,
                "plan": "企业版",
                "usage": ["windowDays": 7, "requests": 1280, "credits": 9988.5, "truncated": false],
                "capturedAt": "2026-11-14T14:04:00Z",
            ]),
            WorkerQuotaRow(json: [
                "accountId": "acc-wb-3",
                "provider": "workbuddy",
                "nickname": "失效备用账号",
                "source": "own",
                "status": "unauthorized",
                "error": "授权已过期，需要重新登录",
                "capturedAt": "2026-11-13T08:00:00Z",
            ]),
            // 同一行既有余额又有每日奖励：奖励不得进入余额列。
            WorkerQuotaRow(json: [
                "accountId": "qsa-cn-2",
                "provider": "qoder_cn",
                "nickname": "Qoder CN 备用",
                "source": "own",
                "status": "ok",
                "unit": "credits",
                "total": 500,
                "remaining": 320,
                "usage": ["windowDays": 7, "requests": 12, "credits": 88.5, "truncated": false],
                "credits": ["state": "already", "supported": true, "today": ["credits": 50, "claimStatus": "claimed"]],
                "capturedAt": "2026-11-14T14:01:00Z",
            ]),
        ].compactMap { $0 }

        fixture.wbSwitchNotice =
            "已写入并重启客户端，但没能确认客户端采用了它；请打开客户端确认，需要的话恢复原账号"

        // PR #98：奖励状态样本，挂到国内版主账号上（acc-wb-1）。
        if let reward = RewardStatusResult.mock(accountId: "acc-wb-1").account("acc-wb-1") {
            fixture.rewardStates["acc-wb-1"] = reward
        }

        // PR #103：成长活跃样本（已在 / 未活跃 / 疑似延迟 / 未知 四种现状）。
        fixture.activity = .mock()

        // WB-SWITCH-1 / #112：客户端当前登录 = 一次 Magpie 临时登录（acc-wb-2），
        // 原账号是主账号（acc-wb-1）。左栏要同时出现"临时登录 + 到期 + 恢复原账号"。
        fixture.wbClientStatus = WorkbuddyClientStatus(json: [
            "client": [
                "installed": true,
                "running": true,
                "authFile": "~/Library/Application Support/CodeBuddyExtension/Data/Public/auth/workbuddy-desktop.info",
                "authFileExists": true,
                "logoutMarker": false,
            ],
            "live": [
                "uid": "wb_1122334455667788",
                "loggedIn": true,
                "tokenForm": "plaintext",
                "accountsInFile": 2,
                "temporary": true,
                "temporaryAccountId": "acc-wb-2",
                "accessExpiresAt": ConsoleVisualFixture.iso(now, 2400),
                "remainingSeconds": 2400,
                "safeRestoreUntil": ConsoleVisualFixture.iso(now, 1500),
            ],
            "candidates": [
                ["accountId": "acc-wb-1", "uid": "wb_9f3c1d7a4b2e5f6089abcdef12345678",
                 "nickname": "主账号", "switchable": true, "reason": "ok",
                 "mode": "own_refresh", "temporary": false],
                ["accountId": "acc-wb-2", "uid": "wb_1122334455667788",
                 "nickname": "设计外包", "switchable": false, "reason": "live",
                 "mode": "", "temporary": false],
                ["accountId": "acc-wb-ai-1", "uid": "wbai_7788aabbccdd",
                 "nickname": "International Demo", "switchable": false,
                 "reason": "unsupported_provider", "mode": "", "temporary": false],
                ["accountId": "acc-bulk-1", "uid": "wb_bulk_0001",
                 "nickname": "批量账号 1", "switchable": true, "reason": "ok",
                 "mode": "own_refresh", "temporary": false],
                ["accountId": "acc-mag-1", "uid": "wb_mag_0001",
                 "nickname": "Magpie 导入账号", "switchable": true, "reason": "ok",
                 "mode": "magpie_access_temporary", "temporary": true,
                 "accessExpiresAt": ConsoleVisualFixture.iso(now, 3000),
                 "remainingSeconds": 3000],
            ],
            "pendingSwitch": NSNull(),
            "temporaryRestore": [
                "id": "j-demo",
                "targetAccountId": "acc-wb-2",
                "targetUid": "wb_1122334455667788",
                "previousUid": "wb_9f3c1d7a4b2e5f6089abcdef12345678",
                "phase": "verified",
                "authSource": "magpie",
                "temporary": true,
                "accessExpiresAt": ConsoleVisualFixture.iso(now, 2400),
                "restoreState": "pending",
                "authAccepted": "accepted",
                "detail": "客户端已启动并重写了认证文件（说明它接受了这次登录）",
                "createdAt": "2026-11-14T13:58:00Z",
                "activityBefore": ["state": "ok", "diagnosis": "inactive",
                                   "serverDate": "2026-11-14", "todayActive": false, "streakDays": 0],
                "activityAfter": ["state": "ok", "diagnosis": "inactive",
                                  "serverDate": "2026-11-14", "todayActive": false, "streakDays": 0],
            ],
            "restoreAvailable": true,
            "lastSwitch": [
                "id": "j-demo",
                "targetAccountId": "acc-wb-2",
                "targetUid": "wb_1122334455667788",
                "previousUid": "wb_9f3c1d7a4b2e5f6089abcdef12345678",
                "phase": "verified",
                "authSource": "magpie",
                "temporary": true,
                "accessExpiresAt": ConsoleVisualFixture.iso(now, 2400),
                "restoreState": "pending",
                "authAccepted": "accepted",
                "createdAt": "2026-11-14T13:58:00Z",
            ],
        ])
        fixture.wbSwitchNotice =
            "已切换，客户端已接受这个登录（重启后仍然是它）；临时登录有效至 11-14 14:38"
            + "（剩余 40 分钟），到期前请点「恢复原账号」"

        // WB-ROTATE-1：22:30 → 次日 07:00，两个账号，当前停在第二个。
        fixture.wbRotationStatus = WorkbuddyRotationStatus(
            json: ConsoleVisualFixture.rotationStatusJSON(
                now: now,
                state: "dwelling",
                detail: nil,
                restoreResult: nil
            )
        )
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

    static var loading: ConsoleVisualFixture {
        var fixture = empty
        fixture.lastAccountsRefresh = nil
        fixture.isLoading = true
        return fixture
    }

    /// 30 天用量窗口：余额不变，只有用量统计变化。
    static var quota30: ConsoleVisualFixture {
        var fixture = sample
        fixture.workerQuotaWindowDays = 30
        fixture.workerQuotaRows = [
            WorkerQuotaRow(json: [
                "accountId": "acc-wb-1",
                "provider": "workbuddy",
                "nickname": "王翊 · 融影（上海）实业有限公司市场部日常运营主账号",
                "source": "own",
                "status": "ok",
                "unit": "credits",
                "total": 5000,
                "remaining": 4597.75,
                "plan": "标准版",
                "usage": ["windowDays": 30, "requests": 812, "credits": 5043.2, "truncated": false],
                "capturedAt": "2026-11-14T14:02:11Z",
            ]),
            WorkerQuotaRow(json: [
                "accountId": "qsa-cn-1",
                "provider": "qoder_cn",
                "nickname": "Qoder CN 主账号",
                "source": "own",
                "status": "ok",
                "unit": "credits",
                "total": 500,
                "remaining": 320,
                "usage": ["windowDays": 30, "requests": 96, "credits": 388.5, "truncated": false],
                "credits": ["state": "already", "supported": true, "today": ["credits": 50]],
                "capturedAt": "2026-11-14T14:00:00Z",
            ]),
        ].compactMap { $0 }
        return fixture
    }

    /// 旧快照：Magpie 整份数据过期，每行都应显示"旧快照"。
    static var quotaAllStale: ConsoleVisualFixture {
        var fixture = sample
        if let snapshot = fixture.snapshot {
            var stale = snapshot
            stale.stale = true
            stale.state = "Cached"
            stale.accounts = stale.accounts.map { card in
                var copy = card
                copy.stale = true
                return copy
            }
            fixture.snapshot = stale
        }
        return fixture
    }

    /// 只留 WorkBuddy 一张卡：在 960×640 下专门验收长名称与长数值。
    static var quotaMagpieLongOnly: ConsoleVisualFixture {
        var fixture = sample
        if let snapshot = fixture.snapshot {
            var only = snapshot
            only.providers = ["workbuddy"]
            only.accounts = snapshot.accounts.filter { $0.provider == "workbuddy" }
            only.stale = false
            fixture.snapshot = only
        }
        return fixture
    }

    /// 异常授权样本：acc-bulk-1 令牌已过期且不是当前登录账号。
    /// 与「当前登录 = acc-wb-2」的正常样本刻意分开，避免两张截图冒充两种状态。
    static var expiredAuth: ConsoleVisualFixture {
        var fixture = sample
        fixture.accounts = fixture.accounts.map { account in
            guard account.id == "acc-bulk-1" else { return account }
            return ConsoleAccount(
                id: "acc-bulk-1",
                platform: "workbuddy",
                provider: "workbuddy",
                uid: "wb_bulk_0001",
                nickname: "批量账号 1",
                authStatus: "expired",
                checkinEnabled: true,
                updatedAt: "2026-11-10T08:00:00Z",
                tokenExpiresAt: "2026-11-12T08:00:00Z",
                authSource: "own",
                authorizations: [
                    ConsoleAuthorization(
                        id: "auth-bulk-1",
                        kind: "own",
                        status: "expired",
                        tokenExpiresAt: "2026-11-12T08:00:00Z"
                    ),
                ].compactMap { $0 }
            )
        }
        return fixture
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
          "updated_at": "11-21 12:00",
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
            },
            {
              "id": "magpie-wb-1", "provider": "workbuddy", "provider_name": "WorkBuddy",
              "account_label": "王翊 · 融影（上海）实业有限公司市场部日常运营主账号",
              "kind": "subscription", "plan": "标准版",
              "items": [
                {"name": "5 小时窗口", "used": 6.0, "remaining": 94.0, "remaining_text": "4705.75",
                 "display": null, "amount": 294.25, "limit": 5000, "unit": "credits",
                 "unlimited": false, "reset": "2026-11-21 18:00", "resets_at": null, "pool": null},
                {"name": "周窗口", "used": 8.0, "remaining": 92.0, "remaining_text": "92%",
                 "display": null, "amount": null, "limit": null, "unit": null,
                 "unlimited": false, "reset": "2026-11-24 09:00", "resets_at": null, "pool": null}
              ],
              "balance": null, "balance_parts": [], "resets_count": 2, "renew": null,
              "last_served_at": null, "last": false, "failed": false, "deferred": false,
              "stale": false, "read_at": null
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
        // 恢复失败是另一套状态：单独一个 store，避免把正常样本改脏。
        let recoveryStore = ConsoleStore(worker: AccountWorkerManager.shared, settings: settings)
        recoveryStore.applyVisualFixture(.rotationRecovery)
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

        // 异常授权样本：与「当前登录」分开，避免两张相同截图冒充两种状态。
        let expiredStore = ConsoleStore(worker: AccountWorkerManager.shared, settings: settings)
        expiredStore.applyVisualFixture(.expiredAuth)

        // 额度页专属状态：30 天窗口、整份旧快照、空/错误/加载。
        let quota30Store = ConsoleStore(worker: AccountWorkerManager.shared, settings: settings)
        quota30Store.applyVisualFixture(.quota30)
        let quotaStaleStore = ConsoleStore(worker: AccountWorkerManager.shared, settings: settings)
        quotaStaleStore.applyVisualFixture(.quotaAllStale)
        let quotaErrorStore = ConsoleStore(worker: AccountWorkerManager.shared, settings: settings)
        quotaErrorStore.applyVisualFixture(.failure)
        let quotaLoadingStore = ConsoleStore(worker: AccountWorkerManager.shared, settings: settings)
        quotaLoadingStore.applyVisualFixture(.loading)
        let quotaLongStore = ConsoleStore(worker: AccountWorkerManager.shared, settings: settings)
        quotaLongStore.applyVisualFixture(.quotaMagpieLongOnly)

        // 关键状态与弹层
        // 用闭包延迟构造：视图必须在渲染前一刻创建，否则 QA 钩子读到的会是最后一组参数。
        let extras: [(String, () -> AnyView, CGSize, ColorScheme)] = [
            // 恢复失败落在 lastRun（current 为 nil）时的账号页：警示区与「去处理」必须出现。
            ("console-accounts-recovery-1120x760-light",
             { AnyView(consoleView(store: recoveryStore, settings: settings, scheme: .light,
                                   page: .accounts)) },
             CGSize(width: 1120, height: 760), .light),
            // 总览恢复异常：冷启动读到的就是 lastRun=recovery_required。
            ("console-overview-recovery-1120x760-light",
             { AnyView(consoleView(store: recoveryStore, settings: settings, scheme: .light,
                                   page: .overview)) },
             CGSize(width: 1120, height: 760), .light),
            ("console-overview-recovery-1120x760-dark",
             { AnyView(consoleView(store: recoveryStore, settings: settings, scheme: .dark,
                                   page: .overview)) },
             CGSize(width: 1120, height: 760), .dark),
            // 额度页关键状态：30 天窗口、整份旧快照、账号额度为空/错误/加载。
            ("quota-accounts-30d-1120x760-light",
             { AnyView(consoleView(store: quota30Store, settings: settings, scheme: .light,
                                   page: .quota, quotaSource: "accounts")) },
             CGSize(width: 1120, height: 760), .light),
            ("quota-accounts-30d-1120x760-dark",
             { AnyView(consoleView(store: quota30Store, settings: settings, scheme: .dark,
                                   page: .quota, quotaSource: "accounts")) },
             CGSize(width: 1120, height: 760), .dark),
            ("quota-magpie-stale-1120x760-light",
             { AnyView(consoleView(store: quotaStaleStore, settings: settings, scheme: .light,
                                   page: .quota)) },
             CGSize(width: 1120, height: 760), .light),
            ("quota-magpie-stale-1120x760-dark",
             { AnyView(consoleView(store: quotaStaleStore, settings: settings, scheme: .dark,
                                   page: .quota)) },
             CGSize(width: 1120, height: 760), .dark),
            ("quota-accounts-error-1120x760-light",
             { AnyView(consoleView(store: quotaErrorStore, settings: settings, scheme: .light,
                                   page: .quota, quotaSource: "accounts")) },
             CGSize(width: 1120, height: 760), .light),
            ("quota-accounts-loading-1120x760-light",
             { AnyView(consoleView(store: quotaLoadingStore, settings: settings, scheme: .light,
                                   page: .quota, quotaSource: "accounts")) },
             CGSize(width: 1120, height: 760), .light),
            // 账号额度 3 尺寸 × 浅深色。
            ("quota-accounts-960x640-light",
             { AnyView(consoleView(store: store, settings: settings, scheme: .light,
                                   page: .quota, quotaSource: "accounts")) },
             CGSize(width: 960, height: 640), .light),
            ("quota-accounts-960x640-dark",
             { AnyView(consoleView(store: store, settings: settings, scheme: .dark,
                                   page: .quota, quotaSource: "accounts")) },
             CGSize(width: 960, height: 640), .dark),
            ("quota-accounts-1120x760-light",
             { AnyView(consoleView(store: store, settings: settings, scheme: .light,
                                   page: .quota, quotaSource: "accounts")) },
             CGSize(width: 1120, height: 760), .light),
            ("quota-accounts-1120x760-dark",
             { AnyView(consoleView(store: store, settings: settings, scheme: .dark,
                                   page: .quota, quotaSource: "accounts")) },
             CGSize(width: 1120, height: 760), .dark),
            ("quota-accounts-1440x900-light",
             { AnyView(consoleView(store: store, settings: settings, scheme: .light,
                                   page: .quota, quotaSource: "accounts")) },
             CGSize(width: 1440, height: 900), .light),
            ("quota-accounts-1440x900-dark",
             { AnyView(consoleView(store: store, settings: settings, scheme: .dark,
                                   page: .quota, quotaSource: "accounts")) },
             CGSize(width: 1440, height: 900), .dark),
            ("quota-accounts-filter-qoder-1120x760-light",
             { AnyView(consoleView(store: store, settings: settings, scheme: .light,
                                   page: .quota, quotaSource: "accounts", quotaProvider: "qoder_cn")) },
             CGSize(width: 1120, height: 760), .light),
            ("quota-magpie-filter-claude-1120x760-light",
             { AnyView(consoleView(store: store, settings: settings, scheme: .light,
                                   page: .quota, quotaProvider: "claude")) },
             CGSize(width: 1120, height: 760), .light),
            // 960 最小窗口下只留 WorkBuddy 长名称/长数值卡。
            ("quota-magpie-workbuddy-960x640-light",
             { AnyView(consoleView(store: quotaLongStore, settings: settings, scheme: .light,
                                   page: .quota)) },
             CGSize(width: 960, height: 640), .light),
            ("quota-magpie-workbuddy-960x640-dark",
             { AnyView(consoleView(store: quotaLongStore, settings: settings, scheme: .dark,
                                   page: .quota)) },
             CGSize(width: 960, height: 640), .dark),
            ("accounts-qoder-detail-1120x760-light",
             { AnyView(consoleView(store: store, settings: settings, scheme: .light,
                                   page: .accounts, account: "qoderService:qsa-cn-1")) },
             CGSize(width: 1120, height: 760), .light),
            ("console-accounts-long-name-1120x760-light",
             { AnyView(consoleView(store: store, settings: settings, scheme: .light,
                                   page: .accounts, account: "workbuddy:acc-bulk-8")) },
             CGSize(width: 1120, height: 760), .light),
            ("console-accounts-current-1120x760-light",
             { AnyView(consoleView(store: store, settings: settings, scheme: .light,
                                   page: .accounts, account: "workbuddy:acc-wb-2")) },
             CGSize(width: 1120, height: 760), .light),
            ("console-accounts-current-1120x760-dark",
             { AnyView(consoleView(store: store, settings: settings, scheme: .dark,
                                   page: .accounts, account: "workbuddy:acc-wb-2")) },
             CGSize(width: 1120, height: 760), .dark),
            ("console-accounts-expired-1120x760-light",
             { AnyView(consoleView(store: expiredStore, settings: settings, scheme: .light,
                                   page: .accounts, account: "workbuddy:acc-bulk-1")) },
             CGSize(width: 1120, height: 760), .light),
            ("console-accounts-expired-1120x760-dark",
             { AnyView(consoleView(store: expiredStore, settings: settings, scheme: .dark,
                                   page: .accounts, account: "workbuddy:acc-bulk-1")) },
             CGSize(width: 1120, height: 760), .dark),
            ("rewards-block-760x560-light",
             { AnyView(Self.rewardsBlock()) }, CGSize(width: 760, height: 560), .light),
            ("rewards-block-stale-760x560-light",
             { AnyView(Self.rewardsBlock(failure: "读取超时（60s）：account worker 未在时限内返回")) },
             CGSize(width: 760, height: 640), .light),
            ("rotation-sheet-560x660-light",
             { AnyView(ConsoleWorkbuddyRotationSheet(store: store)) },
             CGSize(width: 560, height: 660), .light),
            ("rotation-sheet-560x660-dark",
             { AnyView(ConsoleWorkbuddyRotationSheet(store: store)) },
             CGSize(width: 560, height: 660), .dark),
            ("rotation-history-560x760-light",
             { AnyView(ConsoleWorkbuddyRotationSheet(
                 store: store, expandedHistory: true, focus: .history
             )) },
             CGSize(width: 560, height: 760), .light),
            ("rotation-recovery-560x660-light",
             { AnyView(ConsoleWorkbuddyRotationSheet(store: recoveryStore, focus: .status)) },
             CGSize(width: 560, height: 660), .light),
            ("activity-block-1000x420-light",
             { AnyView(Self.activityBlock()) }, CGSize(width: 1000, height: 420), .light),
            ("activity-block-1000x420-dark",
             { AnyView(Self.activityBlock()) }, CGSize(width: 1000, height: 420), .dark),
            ("rewards-block-760x560-dark",
             { AnyView(Self.rewardsBlock()) }, CGSize(width: 760, height: 560), .dark),
            ("accounts-profile-detail-1120x760-dark",
             { AnyView(consoleView(store: store, settings: settings, scheme: .dark,
                                   page: .accounts, account: "qoderProfile:qp-1")) },
             CGSize(width: 1120, height: 760), .dark),
            // WB-SWITCH-1：可切换账号的详情（「切换到此账号」按钮 + 重试说明）。
            ("wb-switch-1120x760-light",
             { AnyView(consoleView(store: store, settings: settings, scheme: .light,
                                   page: .accounts, account: "workbuddy:acc-mag-1")) },
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

        // 奖励展开态：与默认收起的账号详情刻意区分。
        // QA 钩子只在渲染这两张前后生效；生产构建永远默认收起。
        UserDefaults.standard.set(true, forKey: "AICCConsoleExpandRewards")
        for scheme in [ColorScheme.light, .dark] {
            let view = consoleView(store: store, settings: settings, scheme: scheme,
                                   page: .accounts, account: "acc-wb-1")
                .frame(width: 1120, height: 1500)
            try render(view, size: CGSize(width: 1120, height: 1500), scheme: scheme,
                       to: "\(outputDir)/accounts-rewards-detail-expanded-1120x1500-\(schemeName(scheme)).png")
            count += 1
        }
        UserDefaults.standard.set(false, forKey: "AICCConsoleExpandRewards")

        // 空状态与错误状态
        for (fixtureName, fixture) in [("empty", ConsoleVisualFixture.empty),
                                       ("loading", ConsoleVisualFixture.loading),
                                       ("error", ConsoleVisualFixture.failure)] {
            let stateStore = ConsoleStore(worker: AccountWorkerManager.shared, settings: settings)
            stateStore.applyVisualFixture(fixture)
            for scheme in [ColorScheme.light, .dark] {
                for page in [ConsolePage.overview, .accounts, .checkin, .quota] {
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
        quotaProvider: String? = nil,
        checkinPlatform: String? = nil,
        workbuddyTask: String? = nil
    ) -> some View {
        UserDefaults.standard.set(page?.rawValue, forKey: "AICCConsolePage")
        UserDefaults.standard.set(account, forKey: "AICCConsoleAccount")
        UserDefaults.standard.set(quotaSource, forKey: "AICCConsoleQuotaSource")
        UserDefaults.standard.set(quotaProvider, forKey: "AICCConsoleQuotaProvider")
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


extension ConsoleVisualSmokeMain {
    /// 单独渲染「成长活跃」区块：总览页还有统计卡与服务平台，
    /// 四行表格在整页截图里偏小，所以单独出一张复核列宽与文案。
    static func activityBlock() -> some View {
        ConsoleActivityBlock(
            fleet: .mock(),
            loading: false,
            failure: nil,
            lastRefresh: Date().addingTimeInterval(-90),
            onRefresh: {}
        )
        .padding(20)
        .frame(width: 960, alignment: .leading)
        .background(ConsoleTheme.canvas)
    }

    /// 单独渲染奖励区块：账号详情页太长，这块内容会被挤到折叠线以下，
    /// 所以再做一张只含它的图，用来复核排版与文案。
    static func rewardsBlock(failure: String? = nil) -> some View {
        let state = RewardStatusResult.mock(accountId: "acc-wb-1").account("acc-wb-1")
        return ConsoleRewardsBlock(
            state: state,
            loading: false,
            failure: failure,
            onRefresh: {}
        )
        .padding(18)
        .frame(width: 720, alignment: .leading)
        .background(ConsoleTheme.card, in: RoundedRectangle(cornerRadius: 12))
        .padding(20)
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .background(ConsoleTheme.canvas)
    }
}
#endif
