import XCTest
@testable import AICCCore

/// 签到任务中心（UI V5 Phase 2A-2）的纯展示模型。
///
/// 这组用例守四件事：
/// 1. 状态词映射：未知、待确认、失败、成功不能混淆；
/// 2. 操作可用性：执行中 / 不支持的平台不能出现可执行写操作；
/// 3. 摘要与表格同源：标题数字必须等于行状态计数；
/// 4. 行文案不承载机制说明（机制只进 help）。
final class TaskCenterPresentationTests: XCTestCase {

    // MARK: - 每日签到

    func testCheckinStatusMappingCoversEveryWorkerState() {
        XCTAssertEqual(CheckinState.claimed.taskCenterStatus, .done)
        XCTAssertEqual(CheckinState.already.taskCenterStatus, .done)
        XCTAssertEqual(CheckinState.unchecked.taskCenterStatus, .pending)
        XCTAssertEqual(CheckinState.waiting.taskCenterStatus, .pending)
        XCTAssertEqual(CheckinState.stopped.taskCenterStatus, .pending)
        XCTAssertEqual(CheckinState.busy.taskCenterStatus, .running)
        XCTAssertEqual(CheckinState.unsupported.taskCenterStatus, .unsupported)
        XCTAssertEqual(CheckinState.failed.taskCenterStatus, .failed)
        XCTAssertEqual(CheckinState.unauthorized.taskCenterStatus, .needsLogin)
        XCTAssertEqual(CheckinState.noCredential.taskCenterStatus, .needsLogin)
        // 未知与未开启自动都不是成功，也都不能显示成「已完成」。
        XCTAssertEqual(CheckinState.unknown.taskCenterStatus, .needsConfirm)
        XCTAssertEqual(CheckinState.disabled.taskCenterStatus, .unknown)
    }

    func testCheckinUnknownIsNeverDone() {
        let row = CheckinAccountRow(
            accountId: "acc-1", nickname: "账号", enabled: true, state: .unknown
        )
        let display = CheckinRowDisplay(row: row, acting: nil, batching: false)
        XCTAssertEqual(display.status, .needsConfirm)
        XCTAssertFalse(display.status.isDone)
        XCTAssertTrue(display.status.isProblem)
        XCTAssertEqual(display.detail, "服务端没有给出明确结论")
    }

    func testCheckinRowDetailStaysShortAndHonest() {
        let claimed = CheckinAccountRow(
            accountId: "acc-1", nickname: "账号", enabled: true, state: .claimed, credits: 100
        )
        XCTAssertEqual(CheckinRowDisplay(row: claimed, acting: nil, batching: false).detail, "AICC 已确认")
        XCTAssertEqual(CheckinRowDisplay(row: claimed, acting: nil, batching: false).reward, "+100")
        // 没有 checkedAt 就没有最近执行时间，不能编一个「今天」。
        XCTAssertEqual(CheckinRowDisplay(row: claimed, acting: nil, batching: false).lastRunText, "—")

        let already = CheckinAccountRow(
            accountId: "acc-2", nickname: "账号", enabled: false, state: .already
        )
        XCTAssertEqual(
            CheckinRowDisplay(row: already, acting: nil, batching: false).detail,
            "已由其他客户端签到"
        )

        let aicc = CheckinAccountRow(
            accountId: "acc-3", nickname: "账号", enabled: false,
            state: .already, claimedByAiccToday: true
        )
        XCTAssertEqual(
            CheckinRowDisplay(row: aicc, acting: nil, batching: false).detail,
            "今天已由 AICC 签到"
        )

        // 没有奖励数字就是破折号，不写 +0。
        let noCredits = CheckinAccountRow(
            accountId: "acc-4", nickname: "账号", enabled: true, state: .unchecked
        )
        let display = CheckinRowDisplay(row: noCredits, acting: nil, batching: false)
        XCTAssertEqual(display.reward, "—")
        XCTAssertFalse(display.hasReward)
    }

    func testCheckinBusyRowCannotBeTriggeredAgain() {
        let row = CheckinAccountRow(
            accountId: "acc-1", nickname: "账号", enabled: true, state: .unchecked
        )
        let acting = CheckinRowDisplay(row: row, acting: "acc-1", batching: false)
        XCTAssertTrue(acting.isBusy)
        XCTAssertEqual(acting.status, .running)
        XCTAssertFalse(acting.canRun)
        XCTAssertFalse(acting.canToggleAuto)

        // 另一个账号正在执行时，本行也不能点：原来的全局互斥必须保留。
        let otherBusy = CheckinRowDisplay(row: row, acting: "acc-2", batching: false)
        XCTAssertFalse(otherBusy.canRun)

        let batching = CheckinRowDisplay(row: row, acting: nil, batching: true)
        XCTAssertFalse(batching.canRun)

        let idle = CheckinRowDisplay(row: row, acting: nil, batching: false)
        XCTAssertTrue(idle.canRun)
    }

    func testCheckinUnsupportedAndExpiredCannotStartWrites() {
        let unsupported = CheckinAccountRow(
            accountId: "acc-1", nickname: "账号", enabled: true, state: .unsupported
        )
        let row = CheckinRowDisplay(row: unsupported, acting: nil, batching: false)
        XCTAssertEqual(row.status, .unsupported)
        XCTAssertFalse(row.isActionable)
        XCTAssertFalse(row.canRun)
        XCTAssertEqual(row.blockedReason, "该平台不支持签到")

        let expired = CheckinAccountRow(
            accountId: "acc-2", nickname: "账号", enabled: false, state: .unauthorized
        )
        let expiredRow = CheckinRowDisplay(row: expired, acting: nil, batching: false)
        XCTAssertEqual(expiredRow.status, .needsLogin)
        XCTAssertFalse(expiredRow.canRun)
        XCTAssertNotNil(expiredRow.blockedReason)

        // 失败但 errorKind 是授权失效：这是「需要登录」，不是「执行失败」。
        // 同一行不能既说执行失败、又给一个点了只会拿到 401 的按钮。
        let expiredFailure = CheckinAccountRow(
            accountId: "acc-3", nickname: "账号", enabled: false,
            state: .failed, errorKind: "unauthorized"
        )
        let expiredFailureRow = CheckinRowDisplay(row: expiredFailure, acting: nil, batching: false)
        XCTAssertEqual(expiredFailureRow.status, .needsLogin)
        XCTAssertFalse(expiredFailureRow.canRun)
        XCTAssertEqual(expiredFailureRow.action, .login)

        let failed = CheckinAccountRow(
            accountId: "acc-4", nickname: "账号", enabled: false, state: .failed
        )
        // 其他失败可以重试：不因为上次失败就禁用按钮。
        XCTAssertTrue(CheckinRowDisplay(row: failed, acting: nil, batching: false).canRun)
    }

    func testCheckinAutoToggleRespectsEligibility() {
        // 不支持：不能开启，提示要说明原因。
        let unsupported = CheckinAccountRow(
            accountId: "a", nickname: "账号", enabled: false, state: .unsupported
        )
        let unsupportedRow = CheckinRowDisplay(row: unsupported, acting: nil, batching: false)
        XCTAssertFalse(unsupportedRow.canEnableAuto)
        XCTAssertFalse(unsupportedRow.canToggleAuto)
        XCTAssertEqual(unsupportedRow.autoLockReason, "该平台不支持签到，无法开启自动签到")

        // 授权失效但**此前已开启**：必须允许关闭，不能让人停不下来。
        let expiredOn = CheckinAccountRow(
            accountId: "b", nickname: "账号", enabled: true, state: .unauthorized
        )
        let on = CheckinRowDisplay(row: expiredOn, acting: nil, batching: false)
        XCTAssertFalse(on.canEnableAuto)
        XCTAssertTrue(on.canToggleAuto)
        XCTAssertNil(on.autoLockReason)

        // 授权失效且未开启：不能新开。
        let expiredOff = CheckinAccountRow(
            accountId: "c", nickname: "账号", enabled: false, state: .noCredential
        )
        let off = CheckinRowDisplay(row: expiredOff, acting: nil, batching: false)
        XCTAssertFalse(off.canEnableAuto)
        XCTAssertFalse(off.canToggleAuto)
        XCTAssertEqual(off.autoLockReason, "授权失效：先重新登录，再开启自动签到")

        // 其他失败（网络等）不阻止开启：状态未知不等于没资格。
        let network = CheckinAccountRow(
            accountId: "d", nickname: "账号", enabled: false, state: .failed, errorKind: "network"
        )
        XCTAssertTrue(CheckinRowDisplay(row: network, acting: nil, batching: false).canToggleAuto)

        // 执行中禁止切换。
        let normal = CheckinAccountRow(
            accountId: "e", nickname: "账号", enabled: true, state: .claimed
        )
        let busy = CheckinRowDisplay(row: normal, acting: "e", batching: false)
        XCTAssertFalse(busy.canToggleAuto)
        XCTAssertEqual(busy.autoLockReason, "正在执行，结束后再切换")
        XCTAssertFalse(CheckinRowDisplay(row: normal, acting: nil, batching: true).canToggleAuto)
    }

    // MARK: - 已完成 / 待签到 / 授权失效：操作必须对得上（6.2 / 6.3）

    func testCompletedCheckinOnlyOffersReadOnlyRecheck() {
        let claimedRaw = CheckinAccountRow(
            accountId: "a", nickname: "账号", enabled: true, state: .claimed, credits: 100
        )
        let claimed = CheckinRowDisplay(row: claimedRaw, acting: nil, batching: false)
        XCTAssertEqual(claimed.status, .done)
        // 已完成的行只能复查：文案、RPC 都不能是签到。
        XCTAssertEqual(claimed.action, .recheck)
        XCTAssertEqual(claimed.action.title, "复查")
        XCTAssertFalse(claimed.action.isWrite)
        XCTAssertEqual(claimed.action.rpc, "checkin.status")
        XCTAssertTrue(claimed.canPerformAction)
        XCTAssertTrue(claimed.actionHelp.contains("不会重复签到"))

        let already = CheckinRowDisplay(
            row: CheckinAccountRow(accountId: "b", nickname: "账号", enabled: false, state: .already),
            acting: nil, batching: false
        )
        XCTAssertEqual(already.action, .recheck)

        let unchecked = CheckinRowDisplay(
            row: CheckinAccountRow(accountId: "c", nickname: "账号", enabled: true, state: .unchecked),
            acting: nil, batching: false
        )
        XCTAssertEqual(unchecked.status, .pending)
        XCTAssertEqual(unchecked.action, .signIn)
        XCTAssertTrue(unchecked.action.isWrite)
        XCTAssertEqual(unchecked.action.rpc, "checkin.run")
        XCTAssertTrue(unchecked.canPerformAction)

        // 服务端没有结论：不许显示成功，也不许再提交一次。
        let unknown = CheckinRowDisplay(
            row: CheckinAccountRow(accountId: "d", nickname: "账号", enabled: true, state: .unknown),
            acting: nil, batching: false
        )
        XCTAssertEqual(unknown.status, .needsConfirm)
        XCTAssertEqual(unknown.action, .recheck)
        XCTAssertFalse(unknown.action.isWrite)
        XCTAssertTrue(unknown.actionHelp.contains("结果明确前不会提交"))

        // 从没查过：只读检查，不是签到。
        let never = CheckinRowDisplay(
            row: CheckinAccountRow(accountId: "e", nickname: "账号", enabled: false, state: .disabled),
            acting: nil, batching: false
        )
        XCTAssertEqual(never.status, .unknown)
        XCTAssertEqual(never.action, .check)
        XCTAssertEqual(never.action.rpc, "checkin.status")

        // 网络等非授权失败：给重试，仍然先查后写。
        let failed = CheckinRowDisplay(
            row: CheckinAccountRow(accountId: "f", nickname: "账号", enabled: false,
                                   state: .failed, errorKind: "network"),
            acting: nil, batching: false
        )
        XCTAssertEqual(failed.status, .failed)
        XCTAssertEqual(failed.action, .retry)
        XCTAssertTrue(failed.action.isWrite)

        // 授权失效：只给导航去修授权。
        let expired = CheckinRowDisplay(
            row: CheckinAccountRow(accountId: "g", nickname: "账号", enabled: false, state: .unauthorized),
            acting: nil, batching: false
        )
        XCTAssertEqual(expired.action, .login)
        XCTAssertNil(expired.action.rpc)
        XCTAssertTrue(expired.canPerformAction)

        let unsupported = CheckinRowDisplay(
            row: CheckinAccountRow(accountId: "h", nickname: "账号", enabled: false, state: .unsupported),
            acting: nil, batching: false
        )
        XCTAssertEqual(unsupported.action, CheckinRowAction.none)
        XCTAssertFalse(unsupported.canPerformAction)

        // 执行中：状态列已经写了「执行中」，操作列不再摆第二个入口。
        let busy = CheckinRowDisplay(row: claimedRaw, acting: "a", batching: false)
        XCTAssertEqual(busy.status, .running)
        XCTAssertEqual(busy.action, CheckinRowAction.none)
        XCTAssertFalse(busy.canPerformAction)
    }

    /// 只读动作只允许对应 `checkin.status`；写动作才配 `checkin.run`。
    /// 这条契约是「复查不会绕过服务端规则、也不会偷偷发 POST」的兜底。
    func testOnlyWriteActionsMapToCheckinRun() {
        let actions: [CheckinRowAction] = [.signIn, .retry, .recheck, .check, .login, .none]
        for action in actions {
            XCTAssertEqual(action.isWrite, action.rpc == "checkin.run", "\(action)")
            if !action.isWrite, let rpc = action.rpc {
                XCTAssertEqual(rpc, "checkin.status", "\(action)")
            }
        }
        XCTAssertNil(CheckinRowAction.login.rpc)
        XCTAssertNil(CheckinRowAction.none.rpc)
    }

    func testCheckinSummaryMatchesRows() {
        let rows = [
            CheckinRowDisplay(
                row: CheckinAccountRow(accountId: "a", nickname: "A", enabled: true, state: .claimed),
                acting: nil, batching: false
            ),
            CheckinRowDisplay(
                row: CheckinAccountRow(accountId: "b", nickname: "B", enabled: true, state: .already),
                acting: nil, batching: false
            ),
            CheckinRowDisplay(
                row: CheckinAccountRow(accountId: "c", nickname: "C", enabled: false, state: .unchecked),
                acting: nil, batching: false
            ),
            CheckinRowDisplay(
                row: CheckinAccountRow(accountId: "d", nickname: "D", enabled: false, state: .failed),
                acting: nil, batching: false
            ),
        ]
        // 标题只补摘要里没有的自动状态，不再重复完成数/需处理数。
        XCTAssertEqual(TaskCenterSummary.checkin(rows), "自动 2 个")

        let headline = TaskCenterHeadline.checkin(rows, loading: false)
        XCTAssertEqual(headline.done, "2")
        XCTAssertEqual(headline.pending, "1")
        XCTAssertEqual(headline.problems, "1")
        XCTAssertTrue(headline.hasProblems)
    }

    func testHeadlineDoesNotFakeZeroWhileLoading() {
        let headline = TaskCenterHeadline.checkin([], loading: true)
        XCTAssertEqual(headline.done, "—")
        XCTAssertEqual(headline.pending, "—")
        XCTAssertEqual(headline.problems, "—")
        XCTAssertFalse(headline.hasProblems)

        let empty = TaskCenterHeadline.checkin([], loading: false)
        XCTAssertEqual(empty.done, "0")
        XCTAssertFalse(empty.hasProblems)
        XCTAssertEqual(TaskCenterSummary.checkin([]), "自动未开启")
    }

    // MARK: - 成长中心

    func testGrowthTaskStatusMapping() {
        XCTAssertEqual(GrowthTaskStatus.notAccepted.taskCenterStatus, .pending)
        XCTAssertEqual(GrowthTaskStatus.accepted.taskCenterStatus, .accepted)
        XCTAssertEqual(GrowthTaskStatus.inProgress.taskCenterStatus, .inProgress)
        // 「服务端说完成」是待领奖，不能显示成已领取。
        XCTAssertEqual(GrowthTaskStatus.completed.taskCenterStatus, .claimable)
        XCTAssertEqual(GrowthTaskStatus.claimed.taskCenterStatus, .claimed)
        XCTAssertEqual(GrowthTaskStatus.locked.taskCenterStatus, .locked)
        XCTAssertEqual(GrowthTaskStatus.unknown.taskCenterStatus, .unknown)
    }

    func testGrowthTaskClassLabelOnlyForManualAndUnknown() throws {
        let automatic = try XCTUnwrap(GrowthTaskRow(json: [
            "code": "playbook_prompt", "title": "体验 Playbook",
            "class": "automatic", "rawStatus": "accepted", "rewardCredit": 20,
        ]))
        XCTAssertNil(GrowthTaskDisplay(task: automatic, latestRun: nil).classLabel)

        let unknown = try XCTUnwrap(GrowthTaskRow(json: [
            "code": "mystery", "title": "未知任务", "class": "unknown", "rawStatus": "accepted",
        ]))
        XCTAssertEqual(GrowthTaskDisplay(task: unknown, latestRun: nil).classLabel, "待识别")
        XCTAssertEqual(GrowthTaskDisplay(task: unknown, latestRun: nil).status, .accepted)

        let manual = try XCTUnwrap(GrowthTaskRow(json: [
            "code": "Expert_lighthouse", "title": "灯塔", "class": "manual", "rawStatus": "not_accepted",
        ]))
        XCTAssertEqual(GrowthTaskDisplay(task: manual, latestRun: nil).classLabel, "需人工")
        XCTAssertEqual(GrowthTaskDisplay(task: manual, latestRun: nil).status, .pending)
    }

    func testGrowthProblemTextOnlyForProblemRuns() throws {
        let task = try XCTUnwrap(GrowthTaskRow(json: [
            "code": "Library_read", "title": "阅读资料库", "class": "automatic", "rawStatus": "completed",
        ]))
        let ok = try XCTUnwrap(GrowthRunRecord(json: [
            "taskCode": "Library_read", "action": "claim", "outcome": "ok", "credit": 10,
        ]))
        XCTAssertNil(GrowthTaskDisplay(task: task, latestRun: ok).problemText)

        let failed = try XCTUnwrap(GrowthRunRecord(json: [
            "taskCode": "Library_read", "action": "claim", "outcome": "failed",
            "errorDetail": "任务已下架",
        ]))
        XCTAssertEqual(
            GrowthTaskDisplay(task: task, latestRun: failed).problemText,
            "上次领奖失败：任务已下架"
        )

        let unknown = try XCTUnwrap(GrowthRunRecord(json: [
            "taskCode": "Library_read", "action": "claim", "outcome": "unknown",
        ]))
        XCTAssertEqual(
            GrowthTaskDisplay(task: task, latestRun: unknown).problemText,
            "上次领奖未确认"
        )
    }

    func testGrowthAccountNeedsLoginAndKeepsTechnicalHelpOutOfRows() throws {
        let scan = try XCTUnwrap(GrowthAccountScan(json: [
            "accountId": "acc-1",
            "nickname": "设计外包",
            "state": "unauthorized",
            "authSource": "magpie",
            "pathPrefix": "/v2/activity/growth",
            "errors": [["field": "tasks", "kind": "unauthorized", "message": "401"]],
        ]))
        let display = GrowthAccountDisplay(scan: scan, runs: nil, expanded: false)
        XCTAssertEqual(display.status, .needsLogin)
        XCTAssertEqual(display.tasks.count, 0)
        // 技术信息只进 help。
        XCTAssertTrue(display.help.contains("授权来源：Magpie 授权"))
        XCTAssertTrue(display.help.contains("401"))
        XCTAssertTrue(display.help.contains("/v2/activity/growth"))
        XCTAssertFalse(display.pendingText.contains("401"))
    }

    func testGrowthAccountLastRunTextIsCompact() throws {
        let scan = try XCTUnwrap(GrowthAccountScan(json: [
            "accountId": "acc-1", "nickname": "账号", "state": "ok",
            "counts": ["total": 19, "claimable": 2, "acceptable": 1],
        ]))
        let runs = GrowthRuns(json: [
            "recent": [[
                "taskCode": "Library_read", "action": "claim", "outcome": "ok", "credit": 20,
                "executedAt": "2026-11-14T05:58:00Z",
            ]],
        ])
        let display = GrowthAccountDisplay(scan: scan, runs: runs, expanded: false)
        XCTAssertEqual(display.taskText, "19 个任务")
        XCTAssertEqual(display.pendingText, "2 待领奖")
        XCTAssertTrue(display.pendingIsAttention)
        XCTAssertTrue(display.lastRunText.hasPrefix("领奖成功 · +20"))
    }

    func testGrowthSummarySeparatesScannedAndUnscanned() {
        XCTAssertEqual(
            TaskCenterSummary.growth(auto: GrowthAutoStatus(json: nil), scanned: false),
            "尚未扫描"
        )
        XCTAssertEqual(
            TaskCenterSummary.growth(auto: GrowthAutoStatus(json: nil), scanned: true),
            "仅手动"
        )
    }

    func testGrowthProblemsAgreeBetweenHeadlineAndSectionDetail() throws {
        let ok = try XCTUnwrap(GrowthAccountScan(json: [
            "accountId": "acc-1", "nickname": "账号", "state": "ok",
            "counts": ["claimable": 1],
            "tasks": [[
                "code": "Library_read", "title": "阅读资料库", "class": "automatic",
                "rawStatus": "completed", "rewardCredit": 10,
            ]],
        ]))
        let expired = try XCTUnwrap(GrowthAccountScan(json: [
            "accountId": "acc-2", "nickname": "过期", "state": "unauthorized",
        ]))
        let failedRun = GrowthRuns(json: [
            "latestByTask": [[
                "taskCode": "Library_read", "action": "claim", "outcome": "failed",
                "errorDetail": "任务已下架",
            ]],
        ])
        let rows = [
            GrowthAccountDisplay(scan: ok, runs: failedRun, expanded: false),
            GrowthAccountDisplay(scan: expired, runs: nil, expanded: false),
        ]
        let breakdown = GrowthProblemBreakdown(rows: rows)
        XCTAssertEqual(breakdown.needsLogin, 1)
        XCTAssertEqual(breakdown.accounts, 0)
        XCTAssertEqual(breakdown.tasks, 1)
        XCTAssertEqual(breakdown.total, 2)

        let headline = TaskCenterHeadline.growth(rows, loading: false)
        XCTAssertEqual(headline.problems, "2")
        // 分区标题不再重复异常数，只留自动状态。
        XCTAssertEqual(
            TaskCenterSummary.growth(auto: GrowthAutoStatus(json: nil), scanned: true),
            "仅手动"
        )
    }

    /// 摘要必须由展示行派生：即使 `counts` 与任务列表互相矛盾，也只认列表。
    func testGrowthHeadlineDerivesFromDisplayRows() throws {
        let scan = try XCTUnwrap(GrowthAccountScan(json: [
            "accountId": "acc-1", "nickname": "账号", "state": "ok",
            // 故意与下面的任务列表不一致：摘要不许读这两个数字。
            "counts": ["total": 4, "claimable": 9, "acceptable": 9],
            "tasks": [
                ["code": "a", "title": "A", "class": "automatic", "rawStatus": "claimed"],
                ["code": "b", "title": "B", "class": "automatic", "rawStatus": "completed"],
                ["code": "c", "title": "C", "class": "automatic", "rawStatus": "not_accepted"],
                ["code": "d", "title": "D", "class": "automatic", "rawStatus": "in_progress"],
            ],
        ]))
        let row = GrowthAccountDisplay(scan: scan, runs: nil, expanded: false)
        XCTAssertEqual(row.claimedTaskCount, 1)
        // 待处理 = 待领奖 + 未接受；已接受/进行中不占待处理。
        XCTAssertEqual(row.pendingTaskCount, 2)

        let headline = TaskCenterHeadline.growth([row], loading: false)
        XCTAssertEqual(headline.done, "1")
        XCTAssertEqual(headline.pending, "2")
        XCTAssertEqual(headline.problems, "0")

        // 加载中不伪造 0。
        XCTAssertEqual(TaskCenterHeadline.growth([row], loading: true).done, "—")
    }

    func testGrowthTickIsPrimaryOnlyWhenAutoEnabled() {
        let off = GrowthAutoStatus(json: ["enabled": false])
        XCTAssertEqual(off.tickEmphasis, .secondary)
        XCTAssertEqual(off.modeLabel, "仅手动")
        // 关闭时这一轮是只读的，按钮不能说成「检查」以外的写操作。
        XCTAssertEqual(off.tickTitle, "只读检查")
        XCTAssertTrue(off.tickHelp.contains("只读"))
        XCTAssertFalse(off.tickHelp.contains("接单"))

        let on = GrowthAutoStatus(json: ["enabled": true, "intervalSeconds": 86_400])
        XCTAssertEqual(on.tickEmphasis, .primary)
        XCTAssertEqual(on.modeLabel, "自动管理")
        // 开启时这一轮可能接单/领奖，按钮与 help 必须说出来。
        XCTAssertEqual(on.tickTitle, "接单 / 领奖")
        XCTAssertTrue(on.tickHelp.contains("接单"))
        XCTAssertTrue(on.tickHelp.contains("领取"))
    }

    // MARK: - 旅行

    func testTravelStageMapping() {
        XCTAssertEqual(TravelStatus.Stage.dispatchable.taskCenterStatus(claimed: false), .pending)
        XCTAssertEqual(TravelStatus.Stage.traveling.taskCenterStatus(claimed: false), .inProgress)
        XCTAssertEqual(TravelStatus.Stage.arrived.taskCenterStatus(claimed: false), .claimable)
        XCTAssertEqual(TravelStatus.Stage.arrived.taskCenterStatus(claimed: true), .claimed)
        XCTAssertEqual(TravelStatus.Stage.doneToday.taskCenterStatus(claimed: true), .claimed)
        XCTAssertEqual(TravelStatus.Stage.doneToday.taskCenterStatus(claimed: false), .done)
        XCTAssertEqual(TravelStatus.Stage.noBuddy.taskCenterStatus(claimed: false), .unsupported)
        XCTAssertEqual(TravelStatus.Stage.unsupported.taskCenterStatus(claimed: false), .unsupported)
        XCTAssertEqual(TravelStatus.Stage.failed.taskCenterStatus(claimed: false), .failed)
        XCTAssertEqual(TravelStatus.Stage.unknown.taskCenterStatus(claimed: false), .unknown)
    }

    func testTravelActionsRequireStageAndIdle() throws {
        let arrived = try XCTUnwrap(TravelAccountRow(json: [
            "accountId": "acc-1", "nickname": "账号", "travelEnabled": true,
            "travel": ["label": "arrived", "locationName": "杭州", "claimed": false, "rewardCredits": 30],
        ]))
        let idle = TravelRowDisplay(row: arrived, acting: nil)
        XCTAssertTrue(idle.isClaimable)
        XCTAssertTrue(idle.canClaim)
        XCTAssertFalse(idle.isDispatchable)
        XCTAssertEqual(idle.reward, "+30")
        XCTAssertEqual(idle.rewardNote, "待领取")

        let busy = TravelRowDisplay(row: arrived, acting: "acc-1")
        XCTAssertTrue(busy.isClaimable)
        XCTAssertFalse(busy.canClaim)
        XCTAssertFalse(busy.canToggleAuto)

        let dispatchable = try XCTUnwrap(TravelAccountRow(json: [
            "accountId": "acc-2", "nickname": "账号",
            "travel": ["label": "dispatchable", "locationName": "上海"],
        ]))
        let row = TravelRowDisplay(row: dispatchable, acting: nil)
        XCTAssertTrue(row.isDispatchable)
        XCTAssertTrue(row.canDepart)
        XCTAssertFalse(row.canClaim)
        XCTAssertEqual(row.reward, "—")
    }

    func testTravelUnsupportedHasNoWritableAction() throws {
        let unsupported = try XCTUnwrap(TravelAccountRow(json: [
            "accountId": "acc-3", "nickname": "账号",
            "travel": ["label": "unsupported"],
        ]))
        let row = TravelRowDisplay(row: unsupported, acting: nil)
        XCTAssertEqual(row.status, .unsupported)
        XCTAssertFalse(row.canDepart)
        XCTAssertFalse(row.canClaim)
        XCTAssertTrue(row.statusText.contains("没有成长中心"))
    }

    func testTravelAutoToggleRespectsEligibility() throws {
        // 该档位没有成长中心：不能开启。
        let unsupported = try XCTUnwrap(TravelAccountRow(json: [
            "accountId": "t1", "nickname": "账号", "travelEnabled": false,
            "travel": ["label": "unsupported"],
        ]))
        let row = TravelRowDisplay(row: unsupported, acting: nil)
        XCTAssertFalse(row.canEnableAuto)
        XCTAssertFalse(row.canToggleAuto)
        XCTAssertEqual(row.autoLockReason, "该档位没有成长中心，无法开启自动旅行")

        // 已开启但后来变成不支持：必须允许关闭。
        let unsupportedOn = try XCTUnwrap(TravelAccountRow(json: [
            "accountId": "t2", "nickname": "账号", "travelEnabled": true,
            "travel": ["label": "unsupported"],
        ]))
        XCTAssertTrue(TravelRowDisplay(row: unsupportedOn, acting: nil).canToggleAuto)

        // 「无 Buddy」与「真正不支持」分开：还没生成 Buddy 不改变开启资格。
        let noBuddy = try XCTUnwrap(TravelAccountRow(json: [
            "accountId": "t3", "nickname": "账号", "travelEnabled": false,
            "travel": ["label": "no_buddy"],
        ]))
        let noBuddyRow = TravelRowDisplay(row: noBuddy, acting: nil)
        XCTAssertTrue(noBuddyRow.canEnableAuto)
        XCTAssertTrue(noBuddyRow.canToggleAuto)

        // 这次没查到（failed）也不改变资格，避免把临时错误当成永久不支持。
        let failed = try XCTUnwrap(TravelAccountRow(json: [
            "accountId": "t4", "nickname": "账号", "travelEnabled": false,
            "travel": ["label": "failed", "lastError": "timeout"],
        ]))
        XCTAssertTrue(TravelRowDisplay(row: failed, acting: nil).canEnableAuto)

        // 执行中禁止切换。
        let normal = try XCTUnwrap(TravelAccountRow(json: [
            "accountId": "t5", "nickname": "账号", "travelEnabled": true,
            "travel": ["label": "arrived", "locationName": "杭州"],
        ]))
        let busy = TravelRowDisplay(row: normal, acting: "t5")
        XCTAssertFalse(busy.canToggleAuto)
        XCTAssertEqual(busy.autoLockReason, "正在执行，结束后再切换")
    }

    // MARK: - Qoder Credits

    func testCreditsReportStatusMapping() throws {
        let claimable = QoderCreditsReport(json: [
            "accountId": "q", "state": "claimable", "claimable": 1, "supported": true,
            "today": ["credits": 100, "claimStatus": "claimable"],
        ])
        XCTAssertEqual(claimable.taskCenterStatus, .claimable)

        let already = QoderCreditsReport(json: ["accountId": "q", "state": "already", "supported": true])
        XCTAssertEqual(already.taskCenterStatus, .claimed)

        let nothing = QoderCreditsReport(json: ["accountId": "q", "state": "nothing", "supported": true])
        XCTAssertEqual(nothing.taskCenterStatus, .idle)

        let unauthorized = QoderCreditsReport(json: ["accountId": "q", "state": "unauthorized"])
        XCTAssertEqual(unauthorized.taskCenterStatus, .needsLogin)

        let unsupported = QoderCreditsReport(json: ["accountId": "q", "state": "unsupported"])
        XCTAssertEqual(unsupported.taskCenterStatus, .unsupported)

        let busy = QoderCreditsReport(json: ["accountId": "q", "state": "busy"])
        XCTAssertEqual(busy.taskCenterStatus, .running)

        let error = QoderCreditsReport(json: ["accountId": "q", "state": "error"])
        XCTAssertEqual(error.taskCenterStatus, .failed)

        // 服务端回查确认领取成功：成功不能被写成「可领取」。
        let confirmed = QoderCreditsReport(json: [
            "accountId": "q", "state": "claimable", "claimed": true, "supported": true,
            "after": ["state": "already", "supported": true, "today": ["credits": 100, "claimStatus": "claimed"]],
        ])
        XCTAssertEqual(confirmed.taskCenterStatus, .claimed)
    }

    func testCreditsClaimRequiresConfirmedClaimable() throws {
        let item = try XCTUnwrap(QoderCreditsAutoItem(json: [
            "accountId": "q1", "edition": "qoder_cn", "nickname": "Qoder CN", "supported": true,
        ]))
        // 没查过：不能领，状态是待查询（不是成功、也不是 0）。
        let unknown = CreditsRowDisplay(item: item, report: nil, runs: [], busy: nil)
        XCTAssertEqual(unknown.status, .unknown)
        XCTAssertEqual(unknown.reward, "—")
        XCTAssertFalse(unknown.canClaim)
        XCTAssertTrue(unknown.canCheck)

        let claimableReport = QoderCreditsReport(json: [
            "accountId": "q1", "state": "claimable", "claimable": 1, "supported": true,
            "today": ["credits": 100, "claimStatus": "claimable"],
        ])
        let claimable = CreditsRowDisplay(item: item, report: claimableReport, runs: [], busy: nil)
        XCTAssertEqual(claimable.status, .claimable)
        XCTAssertEqual(claimable.reward, "+100")
        XCTAssertEqual(claimable.rewardNote, "待领取")
        XCTAssertTrue(claimable.canClaim)

        let claimedReport = QoderCreditsReport(json: [
            "accountId": "q1", "state": "claimable", "claimed": true, "supported": true,
            "after": ["state": "already", "supported": true, "today": ["credits": 100]],
        ])
        let claimed = CreditsRowDisplay(item: item, report: claimedReport, runs: [], busy: nil)
        XCTAssertEqual(claimed.status, .claimed)
        XCTAssertFalse(claimed.canClaim)
    }

    func testCreditsUnsupportedDisablesEveryWrite() throws {
        let item = try XCTUnwrap(QoderCreditsAutoItem(json: [
            "accountId": "q2", "edition": "qoder_global", "nickname": "International",
            "autoEnabled": false, "supported": false,
        ]))
        let row = CreditsRowDisplay(item: item, report: nil, runs: [], busy: nil)
        XCTAssertEqual(row.status, .unsupported)
        XCTAssertFalse(row.canCheck)
        XCTAssertFalse(row.canClaim)
        XCTAssertFalse(row.supportsAuto)
        XCTAssertEqual(row.reward, "—")
    }

    /// 自动领取开关：不支持只拦「新开启」，已开启的账号必须还能关。
    func testCreditsAutoToggleKeepsDisablePathAfterLosingEligibility() throws {
        let revoked = try XCTUnwrap(QoderCreditsAutoItem(json: [
            "accountId": "q1", "edition": "qoder_global", "nickname": "Qoder International",
            "autoEnabled": true, "supported": false,
        ]))
        let revokedRow = CreditsRowDisplay(item: revoked, report: nil, runs: [], busy: nil)
        XCTAssertEqual(revokedRow.status, .unsupported)
        XCTAssertTrue(revokedRow.canToggleAuto)
        XCTAssertNil(revokedRow.autoLockReason)

        let never = try XCTUnwrap(QoderCreditsAutoItem(json: [
            "accountId": "q2", "edition": "qoder_global", "nickname": "Qoder International",
            "autoEnabled": false, "supported": false,
        ]))
        let neverRow = CreditsRowDisplay(item: never, report: nil, runs: [], busy: nil)
        XCTAssertFalse(neverRow.canToggleAuto)
        XCTAssertTrue(neverRow.autoLockReason?.contains("无法开启") == true)

        // 执行中：不允许重复提交。
        let busyRow = CreditsRowDisplay(item: revoked, report: nil, runs: [], busy: "q1")
        XCTAssertFalse(busyRow.canToggleAuto)
        XCTAssertEqual(busyRow.autoLockReason, "正在执行，结束后再切换")

        // 支持的账号正常可切。
        let supported = try XCTUnwrap(QoderCreditsAutoItem(json: [
            "accountId": "q3", "edition": "qoder_cn", "nickname": "Qoder CN", "supported": true,
        ]))
        XCTAssertTrue(CreditsRowDisplay(item: supported, report: nil, runs: [], busy: nil).canToggleAuto)
    }

    func testCreditsBusyDisablesActions() throws {
        let item = try XCTUnwrap(QoderCreditsAutoItem(json: [
            "accountId": "q1", "edition": "qoder_cn", "nickname": "Qoder CN", "supported": true,
        ]))
        let report = QoderCreditsReport(json: [
            "accountId": "q1", "state": "claimable", "claimable": 1, "supported": true,
        ])
        let row = CreditsRowDisplay(item: item, report: report, runs: [], busy: "q1")
        XCTAssertTrue(row.isBusy)
        XCTAssertFalse(row.canCheck)
        XCTAssertFalse(row.canClaim)
    }

    func testCreditsRowsKeepAtMostFiveRuns() throws {
        let item = try XCTUnwrap(QoderCreditsAutoItem(json: [
            "accountId": "q1", "edition": "qoder_cn", "nickname": "Qoder CN", "supported": true,
        ]))
        let runs = (0..<8).compactMap { index in
            QoderCreditRun(json: [
                "id": index, "outcome": "ok", "credits": 100,
                "trigger": index % 2 == 0 ? "manual" : "auto",
                "executedAt": "2026-11-14T02:0\(index % 10):00Z",
            ])
        }
        let row = CreditsRowDisplay(item: item, report: nil, runs: runs, busy: nil)
        XCTAssertEqual(row.runs.count, 5)
        XCTAssertEqual(row.lastRunOutcome, "已领取")
        XCTAssertFalse(row.lastRunTime.isEmpty)
    }

    func testCreditsSummarySeparatesClaimableClaimedAndUnsupported() throws {
        let cn = try XCTUnwrap(QoderCreditsAutoItem(json: [
            "accountId": "q1", "edition": "qoder_cn", "nickname": "Qoder CN", "supported": true,
        ]))
        let global = try XCTUnwrap(QoderCreditsAutoItem(json: [
            "accountId": "q2", "edition": "qoder_global", "nickname": "Qoder International", "supported": false,
        ]))
        let rows = [
            CreditsRowDisplay(
                item: cn,
                report: QoderCreditsReport(json: [
                    "accountId": "q1", "state": "claimable", "claimable": 1, "supported": true,
                ]),
                runs: [], busy: nil
            ),
            CreditsRowDisplay(item: global, report: nil, runs: [], busy: nil),
        ]
        XCTAssertEqual(TaskCenterSummary.credits(rows), "2 个账号 · 1 个不支持")
    }

    // MARK: - 多账号与长名称

    func testLongNamesAndOrderAreStable() {
        let longName = "王翊 · 融影（上海）实业有限公司市场部日常运营主账号"
        let first = CheckinAccountRow(
            accountId: "acc-1", nickname: longName, enabled: true, state: .claimed
        )
        let second = CheckinAccountRow(
            accountId: "acc-2", nickname: "设计外包", enabled: false, state: .unchecked
        )
        let fleet = CheckinFleetStatus(json: [
            "accounts": [
                ["accountId": "acc-1", "nickname": longName, "enabled": true, "state": "claimed"],
                ["accountId": "acc-2", "nickname": "设计外包", "enabled": false, "state": "unchecked"],
            ],
        ])
        let rows = CheckinRowDisplay.rows(fleet, acting: nil, batching: false)
        // 展示层不改写名称：截断是视图的职责，help 里必须是原文。
        XCTAssertEqual(rows.count, 2)
        XCTAssertEqual(Set(rows.map(\.name)), Set([longName, "设计外包"]))
        XCTAssertEqual(Set(rows.map(\.accountId)), Set(["acc-1", "acc-2"]))
        // 需要处理的排在已完成的之前（既有排序规则，不能因为改版而变）。
        XCTAssertEqual(rows.first?.accountId, "acc-2")
        XCTAssertEqual(CheckinRowDisplay(row: first, acting: nil, batching: false).name, longName)
        XCTAssertEqual(CheckinRowDisplay(row: second, acting: nil, batching: false).status, .pending)
    }

    // MARK: - 跨页导航

    @MainActor
    func testOpenCheckinKeepsTaskContext() {
        let nav = ConsoleNavigation(page: .overview)
        nav.openCheckin(platform: .qoder)
        XCTAssertEqual(nav.page, .checkin)
        XCTAssertEqual(nav.checkinPlatform, .qoder)

        nav.openCheckin(platform: .workbuddy, workbuddy: .travel)
        XCTAssertEqual(nav.page, .checkin)
        XCTAssertEqual(nav.checkinPlatform, .workbuddy)
        XCTAssertEqual(nav.workbuddyTask, .travel)
    }
}
