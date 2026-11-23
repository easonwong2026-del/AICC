import XCTest
@testable import AICCCore

/// 总览首屏对轮换恢复异常的可见性（#115 复审 P0）。
///
/// 后端只把 `recovery_required` 放在 `lastRun`、`current` 为 nil 是真实情况；
/// 冷启动拿到这份状态后，首屏必须计入待处理，且未知/失败不得显示"一切正常"。
final class ConsoleOverviewAttentionTests: XCTestCase {
    private func status(_ lastRun: [String: Any]?) -> WorkbuddyRotationStatus {
        var json: [String: Any] = [
            "plan": ["enabled": true, "accountIds": ["a"]],
            "client": ["installed": true],
            "live": ["loggedIn": true],
        ]
        if let lastRun { json["lastRun"] = lastRun }
        return WorkbuddyRotationStatus(json: json)
    }

    private func attention(_ rotation: ConsoleRotationAttentionState) -> ConsoleOverviewAttention {
        ConsoleOverviewAttention(
            fleetIssues: 0,
            failureCount: 0,
            accountsError: false,
            qoderError: false,
            rotation: rotation
        )
    }

    /// 冷启动加载策略：首次没有状态就补读；已有/正在读时不重复。
    func testColdStartPolicyRequestsRotationOnlyWhenMissing() {
        XCTAssertTrue(ConsoleColdStartLoadPolicy.needsRotation(status: nil, loading: false))
        XCTAssertFalse(ConsoleColdStartLoadPolicy.needsRotation(status: nil, loading: true))

        let loaded = status([
            "id": "run-1",
            "state": "completed",
            "restoreResult": "restored",
            "steps": [],
        ])
        XCTAssertFalse(ConsoleColdStartLoadPolicy.needsRotation(status: loaded, loading: false))
        XCTAssertFalse(ConsoleColdStartLoadPolicy.needsRotation(status: loaded, loading: true))
    }

    /// 冷启动：只有 lastRun=recovery_required，页面也能取得并显示异常。
    func testColdStartWithOnlyLastRunRecoverySurfacesAttention() {
        let parsed = status([
            "id": "run-1",
            "state": "recovery_required",
            "restoreResult": "snapshot_corrupt",
            "steps": [],
        ])
        let rotation = ConsoleRotationAttentionState(status: parsed, error: nil)
        guard case .needsAction(let run) = rotation else {
            XCTFail("lastRun=recovery_required 必须产生 needsAction")
            return
        }
        XCTAssertEqual(run.id, "run-1")
        XCTAssertEqual(rotation.pendingTitle, "WorkBuddy 轮换未恢复")
        XCTAssertEqual(rotation.pendingDetail, "原账号快照已损坏，未覆盖")

        let summary = attention(rotation)
        XCTAssertEqual(summary.count, 1)
        XCTAssertEqual(summary.headline, "1 项需要处理")
        XCTAssertFalse(summary.statusUnknown)
    }

    /// 还没读到状态：不能宣布一切正常。
    func testLoadingStateIsUnknownNotNormal() {
        let rotation = ConsoleRotationAttentionState(status: nil, error: nil)
        XCTAssertEqual(rotation, .loading)
        let summary = attention(rotation)
        XCTAssertTrue(summary.statusUnknown)
        XCTAssertEqual(summary.headline, "状态待确认")
        XCTAssertNotEqual(summary.headline, "一切正常")
    }

    /// 读取失败：计为需要处理，并且给出可行动标题。
    func testReadFailureIsUnknownAndActionable() {
        let rotation = ConsoleRotationAttentionState(status: nil, error: "rpc timeout")
        XCTAssertEqual(rotation, .unavailable("rpc timeout"))
        XCTAssertEqual(rotation.pendingTitle, "轮换状态无法确认")
        XCTAssertEqual(rotation.pendingDetail, "状态读取失败，请打开轮换面板确认；不会自动恢复")

        let summary = attention(rotation)
        XCTAssertEqual(summary.count, 1)
        XCTAssertEqual(summary.headline, "1 项需要处理")
    }

    /// 有旧状态但本次读取失败：旧状态不能拿来宣布正常。
    func testStaleStatusWithReadErrorIsTreatedAsUnknown() {
        let recovered = status([
            "id": "run-old",
            "state": "completed",
            "restoreResult": "restored",
            "steps": [],
        ])
        let rotation = ConsoleRotationAttentionState(status: recovered, error: "timeout")
        XCTAssertEqual(rotation, .unavailable("timeout"))
    }

    /// 恢复成功（lastRun 收口为 completed）后，警示与计数都消失。
    func testClearStatusAllowsNormalHeadline() {
        let recovered = status([
            "id": "run-1",
            "state": "completed",
            "restoreResult": "restored",
            "steps": [],
        ])
        let rotation = ConsoleRotationAttentionState(status: recovered, error: nil)
        XCTAssertEqual(rotation, .clear)
        let summary = attention(rotation)
        XCTAssertEqual(summary.count, 0)
        XCTAssertFalse(summary.statusUnknown)
        XCTAssertEqual(summary.headline, "一切正常")
    }

    /// 账号列表刷新策略：冷启动 / 重开窗口会补刷过期快照；切回窗口 / 切页不会抢跑冷启动，且满 10 分钟才自动静默刷。
    func testAccountsRefreshPolicyRespectsColdStartAndTTL() {
        let base = Date(timeIntervalSince1970: 1_795_400_000)

        XCTAssertEqual(ConsoleAccountsRefreshPolicy.defaultTTL, 600)

        // 首次挂载：没有历史快照时必刷；加载中不重复。
        XCTAssertTrue(ConsoleAccountsRefreshPolicy.shouldRefreshOnLoad(lastRefresh: nil, loading: false, now: base))
        XCTAssertFalse(ConsoleAccountsRefreshPolicy.shouldRefreshOnLoad(lastRefresh: nil, loading: true, now: base))

        // 重开窗口：未满 10 分钟不刷，满 10 分钟自动重读。
        XCTAssertFalse(ConsoleAccountsRefreshPolicy.shouldRefreshOnLoad(
            lastRefresh: base,
            loading: false,
            now: base.addingTimeInterval(599.9)
        ))
        XCTAssertTrue(ConsoleAccountsRefreshPolicy.shouldRefreshOnLoad(
            lastRefresh: base,
            loading: false,
            now: base.addingTimeInterval(600.0)
        ))

        // 切回窗口 / 切页：冷启动未完成（lastRefresh == nil）时不抢跑；已加载后满 10 分钟才静默刷新。
        XCTAssertFalse(ConsoleAccountsRefreshPolicy.shouldRefreshOnRevisit(lastRefresh: nil, loading: false, now: base))
        XCTAssertFalse(ConsoleAccountsRefreshPolicy.shouldRefreshOnRevisit(
            lastRefresh: base,
            loading: true,
            now: base.addingTimeInterval(900)
        ))
        XCTAssertFalse(ConsoleAccountsRefreshPolicy.shouldRefreshOnRevisit(
            lastRefresh: base,
            loading: false,
            now: base.addingTimeInterval(599.9)
        ))
        XCTAssertTrue(ConsoleAccountsRefreshPolicy.shouldRefreshOnRevisit(
            lastRefresh: base,
            loading: false,
            now: base.addingTimeInterval(600.0)
        ))
    }
}
