import Foundation
import XCTest
@testable import AICCCore

/// 异步结果与历史状态的一致性。
///
/// 这三段流程就是 `ConsoleStore` 实际调用的代码：预演绑定发起时的计划、
/// 轮换读取失败必须记录、Credits 读取失败保留上次成功快照。
@MainActor
final class AutomationAsyncFlowTests: XCTestCase {

    // MARK: - P0 预演绑定发起时的计划

    /// 发起 A 的预演 → 期间保存 B → A 返回：A 的结果必须被丢弃。
    func testPreviewIsDiscardedWhenThePlanChangedDuringTheRequest() async {
        let coordinator = RotationPreviewCoordinator()
        var generation = 0

        let result = await RotationPreviewFlow.run(
            coordinator: coordinator,
            planKey: "plan-A",
            generation: generation,
            currentGeneration: { generation },
            fetch: {
                // 模拟请求往返期间保存了另一份计划。
                generation += 1
                return Self.dryRun("A")
            }
        )

        XCTAssertEqual(result, .planChanged)
        XCTAssertNotEqual(result, .applied(Self.dryRun("A"), planKey: "plan-A"))
    }

    /// A 的预演还没回来，B 的预演先落地：A 返回时不能覆盖 B。
    func testAnOlderPreviewCannotOverwriteANewerResult() async {
        let coordinator = RotationPreviewCoordinator()
        let gate = PreviewGate()

        let slow = Task { @MainActor in
            await RotationPreviewFlow.run(
                coordinator: coordinator,
                planKey: "plan-A",
                generation: 0,
                currentGeneration: { 0 },
                fetch: { try await withCheckedThrowingContinuation { gate.continuation = $0 } }
            )
        }

        for _ in 0..<2000 where gate.continuation == nil {
            await Task.yield()
        }
        XCTAssertNotNil(gate.continuation, "慢请求应当已经进入 fetch")

        let fast = await RotationPreviewFlow.run(
            coordinator: coordinator,
            planKey: "plan-B",
            generation: 0,
            currentGeneration: { 0 },
            fetch: { Self.dryRun("B") }
        )
        XCTAssertEqual(fast, .applied(Self.dryRun("B"), planKey: "plan-B"))

        gate.continuation?.resume(returning: Self.dryRun("A"))
        let slowResult = await slow.value
        XCTAssertEqual(slowResult, .superseded, "旧请求返回后不能覆盖 B 的结果")
    }

    func testPreviewFailureIsReported() async {
        let coordinator = RotationPreviewCoordinator()
        let result = await RotationPreviewFlow.run(
            coordinator: coordinator,
            planKey: "plan-A",
            generation: 0,
            currentGeneration: { 0 },
            fetch: { throw FlowTestError.boom }
        )
        XCTAssertEqual(result, .failed("助手未运行"))
    }

    func testPreviewWithoutASavedPlanNeverCallsTheServer() async {
        let coordinator = RotationPreviewCoordinator()
        var fetches = 0
        let result = await RotationPreviewFlow.run(
            coordinator: coordinator,
            planKey: nil,
            generation: 0,
            currentGeneration: { 0 },
            fetch: { fetches += 1; return Self.dryRun("A") }
        )
        XCTAssertEqual(result, .failed("还没有可预演的计划。"))
        XCTAssertEqual(fetches, 0)
    }

    // MARK: - P1 静默刷新也必须记录失败

    func testWorkerDownWithCacheKeepsItButMarksItStale() async {
        let cached = Self.status(enabled: true)
        let outcome = await RotationRefreshFlow.run(
            cached: cached,
            workerMessage: "助手未运行",
            ensureWorker: { false },
            fetch: {
                XCTFail("Worker 不可用时不应该发读取请求")
                return cached
            }
        )

        XCTAssertEqual(outcome.status, cached, "保留最后成功快照")
        XCTAssertEqual(outcome.error, "助手未运行", "静默刷新也必须记录失败")

        let overview = RotationOverview.make(status: outcome.status, error: outcome.error)
        XCTAssertTrue(overview.state.isStale)
        XCTAssertEqual(overview.state.tone, .warning)
        XCTAssertTrue(overview.compactSummary.contains("当前无法确认"))
    }

    func testWorkerDownWithoutCacheIsAFailure() async {
        let outcome = await RotationRefreshFlow.run(
            cached: nil,
            workerMessage: "助手未运行",
            ensureWorker: { false },
            fetch: { throw FlowTestError.boom }
        )
        XCTAssertNil(outcome.status)
        XCTAssertEqual(outcome.error, "助手未运行")

        let overview = RotationOverview.make(status: outcome.status, error: outcome.error)
        XCTAssertEqual(overview.state, .failed("助手未运行"))
        XCTAssertNotEqual(overview.state, .off)
    }

    func testRefreshRecoveryClearsTheStaleMark() async {
        let fresh = Self.status(enabled: true)
        let outcome = await RotationRefreshFlow.run(
            cached: Self.status(enabled: false),
            workerMessage: "助手未运行",
            ensureWorker: { true },
            fetch: { fresh }
        )
        XCTAssertEqual(outcome.status, fresh)
        XCTAssertNil(outcome.error)

        let overview = RotationOverview.make(status: outcome.status, error: outcome.error)
        XCTAssertEqual(overview.state, .on)
        XCTAssertFalse(overview.state.isStale)
    }

    // MARK: - P1 Credits 保留真实历史

    func testCreditsFailureKeepsTheLastSuccessfulSnapshot() async {
        let cached = [
            Self.credit("a", enabled: true, supported: true),
            Self.credit("b", enabled: true, supported: true),
        ]
        let outcome = await QoderCreditsRefreshFlow.run(
            cached: cached,
            loaded: true,
            fetch: { throw FlowTestError.boom }
        )

        XCTAssertEqual(outcome.items, cached, "读取失败不能清空上次成功快照")
        XCTAssertTrue(outcome.loaded)
        XCTAssertEqual(outcome.error, "助手未运行")

        let row = AutomationPlanBuilder.credits(Self.creditsInputs(outcome), now: Date())
        XCTAssertTrue(row.state.isStale)
        XCTAssertEqual(row.statusText, "上次：已开启")
        XCTAssertTrue(row.summary.contains("2/2 个账号已开启"), row.summary)
    }

    func testCreditsFailureWithoutAnySuccessfulReadIsAFailure() async {
        let outcome = await QoderCreditsRefreshFlow.run(
            cached: [],
            loaded: false,
            fetch: { throw FlowTestError.boom }
        )
        XCTAssertTrue(outcome.items.isEmpty)
        XCTAssertFalse(outcome.loaded)

        let row = AutomationPlanBuilder.credits(Self.creditsInputs(outcome), now: Date())
        guard case .failed = row.state else {
            return XCTFail("从未成功读取时必须是读取失败")
        }
        XCTAssertEqual(row.statusText, "读取失败")
    }

    func testCreditsPreviousNoSupportedAccountStaysHistorical() async {
        let cached = [Self.credit("a", enabled: false, supported: false)]
        let outcome = await QoderCreditsRefreshFlow.run(
            cached: cached,
            loaded: true,
            fetch: { throw FlowTestError.boom }
        )

        let row = AutomationPlanBuilder.credits(Self.creditsInputs(outcome), now: Date())
        XCTAssertTrue(row.state.isStale)
        XCTAssertEqual(row.statusText, "上次：未开启")
        XCTAssertTrue(row.summary.contains("暂无支持账号"), row.summary)
    }

    func testCreditsRecoveryClearsTheStaleMark() async {
        let fresh = [Self.credit("a", enabled: true, supported: true)]
        let outcome = await QoderCreditsRefreshFlow.run(
            cached: [Self.credit("a", enabled: false, supported: true)],
            loaded: true,
            fetch: { fresh }
        )
        XCTAssertEqual(outcome.items, fresh)
        XCTAssertNil(outcome.error)

        let row = AutomationPlanBuilder.credits(Self.creditsInputs(outcome), now: Date())
        XCTAssertEqual(row.state, .on)
        XCTAssertFalse(row.state.isStale)
        XCTAssertFalse(row.summary.contains("当前无法确认"))
    }

    // MARK: - 样本

    private static func creditsInputs(_ outcome: QoderCreditsRefreshOutcome) -> AutomationInputs {
        var inputs = AutomationInputs()
        inputs.creditsAuto = outcome.items
        inputs.creditsLoaded = outcome.loaded
        inputs.creditsError = outcome.error
        return inputs
    }

    private static func credit(_ id: String, enabled: Bool, supported: Bool) -> QoderCreditsAutoItem {
        QoderCreditsAutoItem(json: [
            "accountId": id,
            "autoEnabled": enabled,
            "supported": supported,
        ])!
    }

    private static func status(enabled: Bool) -> WorkbuddyRotationStatus {
        WorkbuddyRotationStatus(json: [
            "plan": [
                "enabled": enabled,
                "timezone": "Asia/Shanghai",
                "weekdays": [1, 2, 3, 4, 5],
                "startMinutes": 22 * 60 + 30,
                "endMinutes": 7 * 60,
                "accountIds": ["a", "b"],
                "dwellSeconds": 3600,
            ],
            "schedule": ["inWindow": false],
            "live": ["uid": "wb_1", "loggedIn": true],
            "history": [],
        ])
    }

    private static func dryRun(_ id: String) -> WorkbuddyRotationDryRun {
        WorkbuddyRotationDryRun(json: [
            "steps": [["accountId": id, "outcome": "switch"]],
            "warnings": [],
        ])
    }
}

private enum FlowTestError: LocalizedError {
    case boom
    var errorDescription: String? { "助手未运行" }
}

/// 让慢请求停在 `fetch` 里，测试才能制造乱序返回。
@MainActor
private final class PreviewGate {
    var continuation: CheckedContinuation<WorkbuddyRotationDryRun, Error>?
}
