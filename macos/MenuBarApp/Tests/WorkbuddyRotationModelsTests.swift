import XCTest
@testable import AICCCore

/// 夜间轮换模型的文案与边界。
///
/// 这些断言只盯"会不会说错话"——把"跳过"说成"完成"、把"未知"说成"未活跃"，
/// 都是会让用户做出错误决定的错。
final class WorkbuddyRotationModelsTests: XCTestCase {
    func testPlanDescribesACrossMidnightWindowHonestly() {
        let plan = WorkbuddyRotationPlan(json: [
            "enabled": true,
            "timezone": "Asia/Shanghai",
            "weekdays": [1, 2, 3, 4, 5, 6, 7],
            "startMinutes": 22 * 60 + 30,
            "endMinutes": 7 * 60,
            "accountIds": ["a", "b"],
            "dwellSeconds": 3600,
        ])
        XCTAssertTrue(plan.crossesMidnight)
        XCTAssertEqual(plan.windowText, "22:30 → 次日 07:00")
        XCTAssertEqual(plan.weekdayText, "每天")
        XCTAssertEqual(plan.dwellText, "1 小时")
    }

    func testWeekdayTextNamesTheCommonSets() {
        func plan(_ days: [Int]) -> WorkbuddyRotationPlan {
            WorkbuddyRotationPlan(json: ["weekdays": days, "startMinutes": 0, "endMinutes": 60])
        }
        XCTAssertEqual(plan([1, 2, 3, 4, 5]).weekdayText, "工作日")
        XCTAssertEqual(plan([6, 7]).weekdayText, "周末")
        XCTAssertEqual(plan([1, 3, 5]).weekdayText, "周一、周三、周五")
    }

    func testScheduleSaysSoWhenThereIsNoNextRun() {
        let empty = WorkbuddyRotationSchedule(json: ["inWindow": false])
        XCTAssertEqual(empty.nextRunText, "没有下一次")

        let inside = WorkbuddyRotationSchedule(json: ["inWindow": true])
        XCTAssertEqual(inside.nextRunText, "正在执行窗口内")
    }

    func testStepSeparatesSkippedFromDone() {
        let skipped = WorkbuddyRotationStep(json: [
            "status": "skipped",
            "skipReason": "own_grant_rejected",
        ])
        XCTAssertEqual(skipped.statusText, "已跳过")
        XCTAssertEqual(skipped.skipText, "自有授权被服务端拒绝，需要重新授权")

        let done = WorkbuddyRotationStep(json: ["status": "done"])
        XCTAssertEqual(done.statusText, "已切换")
        XCTAssertNil(done.skipText)
    }

    func testMagpieShortWindowIsExplainedInTermsOfTheDwell() {
        let step = WorkbuddyRotationStep(json: [
            "status": "skipped",
            "skipReason": "magpie_too_short_for_dwell",
        ])
        XCTAssertEqual(step.skipText, "Magpie 令牌剩余时间不够覆盖停留时长")
    }

    func testActivityDistinguishesBeforeAfterAndUnknown() {
        // 还没到停留结束：不能说"未活跃"，只能说还没查。
        let pending = WorkbuddyRotationStep(json: [
            "status": "done",
            "activityBefore": ["state": "ok", "todayActive": false, "streakDays": 1],
        ])
        XCTAssertEqual(pending.activityPhase, .beforeSwitch)
        XCTAssertTrue(pending.activityText.hasPrefix("切换前："), pending.activityText)

        let nothing = WorkbuddyRotationStep(json: ["status": "planned"])
        XCTAssertEqual(nothing.activityPhase, .pendingAfterDwell)
        XCTAssertEqual(nothing.activityText, "停留后尚未查询")

        let after = WorkbuddyRotationStep(json: [
            "status": "done",
            "activityBefore": ["state": "ok", "todayActive": false, "streakDays": 1],
            "activityAfter": ["state": "ok", "todayActive": true, "streakDays": 2],
        ])
        XCTAssertEqual(after.activityPhase, .afterDwell)
        XCTAssertFalse(after.activityText.hasPrefix("切换前"), after.activityText)

        let failed = WorkbuddyRotationStep(json: [
            "status": "done",
            "activityAfter": ["state": "failed", "error": "timeout"],
        ])
        XCTAssertEqual(failed.activityPhase, .failed)
        XCTAssertEqual(failed.activityText, "查询失败／未知")
    }

    func testRunExplainsTheRestoreOutcomeWithoutPretending() {
        let restored = WorkbuddyRotationRun(json: [
            "state": "completed",
            "restoreResult": "restored",
            "steps": [],
        ])
        XCTAssertEqual(restored.restoreText, "已恢复最初账号")
        XCTAssertFalse(restored.needsAttention)

        let corrupt = WorkbuddyRotationRun(json: [
            "state": "recovery_required",
            "restoreResult": "snapshot_corrupt",
            "steps": [],
        ])
        XCTAssertEqual(corrupt.restoreText, "原账号快照已损坏，未覆盖")
        XCTAssertTrue(corrupt.needsAttention)
        XCTAssertEqual(corrupt.stateText, "需要你确认")
    }

    /// P0（UI V5 #115 收口）：恢复失败后 `current` 是 nil、只有 `lastRun` 停在
    /// `recovery_required`。`attentionRun` 必须从 `lastRun` 里把异常捞出来，
    /// 否则总览/账号页会把它显示成"一切正常"。
    func testAttentionRunSurfacesLastRunRecoveryRequired() {
        let needsHelp = WorkbuddyRotationStatus(json: [
            "plan": ["enabled": true, "accountIds": ["a"]],
            "client": ["installed": true],
            "lastRun": [
                "id": "run-1",
                "state": "recovery_required",
                "restoreResult": "snapshot_corrupt",
                "steps": [],
            ],
        ])
        XCTAssertEqual(needsHelp.attentionRun?.id, "run-1")
        XCTAssertEqual(needsHelp.attentionRun?.stateText, "需要你确认")
        XCTAssertTrue(needsHelp.headline.contains("需要你确认"))
    }

    /// 恢复成功后 `lastRun` 收口为 `completed`，警示必须消失，不能一直挂在那。
    func testAttentionRunDisappearsAfterSuccessfulRestore() {
        let recovered = WorkbuddyRotationStatus(json: [
            "plan": ["enabled": true, "accountIds": ["a"]],
            "client": ["installed": true],
            "lastRun": [
                "id": "run-1",
                "state": "completed",
                "restoreResult": "restored",
                "steps": [],
            ],
        ])
        XCTAssertNil(recovered.attentionRun)
        XCTAssertFalse(recovered.headline.contains("需要你确认"))
    }

    /// `current` 自己需要处理时优先于 `lastRun`，两个都是异常也不能互相顶掉。
    func testAttentionRunPrefersCurrentWhenBothNeedAttention() {
        let both = WorkbuddyRotationStatus(json: [
            "plan": ["enabled": true, "accountIds": ["a"]],
            "client": ["installed": true],
            "current": [
                "id": "run-current",
                "state": "recovery_required",
                "restoreResult": "live_changed",
                "steps": [],
            ],
            "lastRun": [
                "id": "run-last",
                "state": "recovery_required",
                "restoreResult": "snapshot_corrupt",
                "steps": [],
            ],
        ])
        XCTAssertEqual(both.attentionRun?.id, "run-current")
    }

    func testHeadlineCoversDisabledEmptyAndUnloggedCases() {
        let disabled = WorkbuddyRotationStatus(json: [
            "plan": ["enabled": false, "accountIds": ["a"]],
            "client": ["installed": true],
            "live": ["loggedIn": true],
        ])
        XCTAssertEqual(disabled.headline, "已关闭")

        let empty = WorkbuddyRotationStatus(json: [
            "plan": ["enabled": true, "accountIds": []],
            "client": ["installed": true],
            "live": ["loggedIn": true],
        ])
        XCTAssertEqual(empty.headline, "还没有选择参与轮换的账号")

        let loggedOut = WorkbuddyRotationStatus(json: [
            "plan": ["enabled": true, "accountIds": ["a"]],
            "client": ["installed": true],
            "live": ["loggedIn": false],
            "schedule": ["inWindow": false],
        ])
        XCTAssertEqual(loggedOut.headline, "客户端当前没有登录，计划不会执行")
    }

    func testDryRunStepTellsSwitchFromSkip() {
        let dry = WorkbuddyRotationDryRun(json: [
            "steps": [
                ["accountId": "a", "nickname": "甲", "outcome": "switch", "temporary": false],
                ["accountId": "b", "nickname": "乙", "outcome": "skip", "detail": "剩余时间不够"],
                ["accountId": "c", "nickname": "丙", "outcome": "already_live"],
            ],
            "warnings": ["每切一个账号都会关闭并重启 WorkBuddy"],
            "originLoggedIn": true,
        ])
        XCTAssertEqual(dry.steps.count, 3)
        XCTAssertTrue(dry.steps[0].summary.contains("甲"))
        XCTAssertTrue(dry.steps[1].summary.contains("跳过"))
        XCTAssertTrue(dry.steps[2].summary.contains("已经登录"))
        XCTAssertEqual(dry.warnings.count, 1)
    }

    /// 恢复失败时后端把这一轮排除在 `current` 之外，只有 `lastRun` 停在待处理。
    /// 界面必须靠 `attentionRun` 同时认这两个位置，否则警示区和入口会一起消失。
    func testRecoveryInLastRunStillNeedsAttention() {
        let status = WorkbuddyRotationStatus(json: [
            "plan": ["enabled": true, "accountIds": ["a"]],
            "lastRun": [
                "id": "r1",
                "state": "recovery_required",
                "detail": "已写回快照，但客户端当前登录是别人",
                "restoreResult": "restore_failed",
            ],
        ])

        XCTAssertNil(status.current, "后端不把 recovery_required 算作活动轮次")
        XCTAssertEqual(status.attentionRun?.id, "r1")
        XCTAssertEqual(status.headline, "已写回快照，但客户端当前登录是别人")

        let healthy = WorkbuddyRotationStatus(json: [
            "plan": ["enabled": true, "accountIds": ["a"]],
            "lastRun": ["id": "r2", "state": "completed", "restoreResult": "restored"],
        ])
        XCTAssertNil(healthy.attentionRun, "正常收尾不该报异常")
    }
}
