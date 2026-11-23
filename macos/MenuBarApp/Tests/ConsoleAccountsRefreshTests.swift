import Foundation
import XCTest
@testable import AICCCore

/// 账号只读刷新状态机、前台 10 分钟调度器与页头状态文案的确定性测试。
@MainActor
final class ConsoleAccountsRefreshTests: XCTestCase {

    private func sampleAccount(_ id: String, nickname: String = "测试号") -> ConsoleAccount {
        ConsoleAccount(
            id: id,
            platform: "workbuddy",
            provider: "workbuddy",
            uid: "uid-\(id)",
            nickname: nickname,
            authStatus: "valid",
            tokenExpiresAt: "2026-12-30T00:00:00Z",
            authSource: "magpie"
        )
    }

    // MARK: - 1. 失败不推进成功时间，区分成功/尝试/错误并标记数据已过期

    func testWorkerUnavailableDoesNotAdvanceLastSuccessAtAndRecordsError() async {
        let base = Date(timeIntervalSince1970: 1_795_400_000)
        var current = base
        let coordinator = ConsoleAccountsRefreshCoordinator(ttl: 600)

        // 先成功读一次
        let first = await coordinator.run(
            trigger: .initialLoad,
            now: { current },
            workerUnavailableMessage: "账号助手未就绪",
            ensureWorker: { true },
            fetch: { [self.sampleAccount("a1")] }
        )
        guard case .succeeded(let initialAccounts, let okAt) = first else {
            return XCTFail("首次刷新应当成功：\(first)")
        }
        XCTAssertEqual(initialAccounts.count, 1)
        XCTAssertEqual(okAt, base)
        XCTAssertEqual(coordinator.state.lastSuccessAt, base)
        XCTAssertEqual(coordinator.state.lastAttemptAt, base)
        XCTAssertNil(coordinator.state.lastError)
        XCTAssertFalse(coordinator.state.isSnapshotStale(now: base.addingTimeInterval(60)))

        // 10 分钟后静默刷新遇到 worker 不可用：成功时间绝不推进，尝试时间更新，记录错误，标记过期
        current = base.addingTimeInterval(600)
        let failed = await coordinator.run(
            trigger: .scheduled,
            now: { current },
            workerUnavailableMessage: "账号助手未就绪",
            ensureWorker: { false },
            fetch: {
                XCTFail("worker 不可用时不应发起 listAccounts RPC")
                return []
            }
        )
        XCTAssertEqual(
            failed,
            .failed(error: "账号助手未就绪", attemptedAt: current, preservedLastSuccessAt: base)
        )
        XCTAssertEqual(coordinator.state.lastSuccessAt, base, "失败绝不能推进成功时间")
        XCTAssertEqual(coordinator.state.lastAttemptAt, current, "尝试时间必须更新")
        XCTAssertEqual(coordinator.state.lastError, "账号助手未就绪", "静默失败也必须记录错误")
        XCTAssertEqual(coordinator.state.consecutiveFailures, 1)
        XCTAssertTrue(coordinator.state.isSnapshotStale(now: current))
    }

    func testFetchThrowPreservesLastSuccessAtAndColdFailureKeepsSuccessNil() async {
        let base = Date(timeIntervalSince1970: 1_795_400_000)
        var current = base
        let coordinator = ConsoleAccountsRefreshCoordinator(ttl: 600)

        // 冷启动失败：lastSuccessAt 必须保持 nil
        let coldFail = await coordinator.run(
            trigger: .initialLoad,
            now: { current },
            workerUnavailableMessage: "助手未运行",
            ensureWorker: { true },
            fetch: { throw RefreshTestError.rpcTimeout }
        )
        XCTAssertEqual(
            coldFail,
            .failed(error: "RPC 读取超时", attemptedAt: base, preservedLastSuccessAt: nil)
        )
        XCTAssertNil(coordinator.state.lastSuccessAt)
        XCTAssertEqual(coordinator.state.lastAttemptAt, base)
        XCTAssertEqual(coordinator.state.lastError, "RPC 读取超时")

        // 手动刷新恢复成功
        current = base.addingTimeInterval(5)
        let recovered = await coordinator.run(
            trigger: .manual,
            now: { current },
            workerUnavailableMessage: "助手未运行",
            ensureWorker: { true },
            fetch: { [self.sampleAccount("a2")] }
        )
        guard case .succeeded(_, let okAt) = recovered else {
            return XCTFail("手动刷新应当成功：\(recovered)")
        }
        XCTAssertEqual(okAt, current)
        XCTAssertEqual(coordinator.state.lastSuccessAt, current)
        XCTAssertNil(coordinator.state.lastError)
        XCTAssertEqual(coordinator.state.consecutiveFailures, 0)

        // 随后手动刷新抛错：保留上次成功时间 okAt
        let failAt = current.addingTimeInterval(20)
        current = failAt
        let secondFail = await coordinator.run(
            trigger: .manual,
            now: { current },
            workerUnavailableMessage: "助手未运行",
            ensureWorker: { true },
            fetch: { throw RefreshTestError.rpcTimeout }
        )
        XCTAssertEqual(
            secondFail,
            .failed(error: "RPC 读取超时", attemptedAt: failAt, preservedLastSuccessAt: okAt)
        )
        XCTAssertEqual(coordinator.state.lastSuccessAt, okAt)
        XCTAssertEqual(coordinator.state.lastAttemptAt, failAt)
        XCTAssertTrue(coordinator.state.isSnapshotStale(now: failAt))
    }

    // MARK: - 2. 防重、失败退避与异步结果防覆盖

    func testConcurrentTriggersAreDeduplicated() async {
        let base = Date(timeIntervalSince1970: 1_795_400_000)
        let coordinator = ConsoleAccountsRefreshCoordinator(ttl: 600)
        let gate = AccountsGate()
        var fetchCalls = 0

        let firstTask = Task { @MainActor in
            await coordinator.run(
                trigger: .initialLoad,
                now: { base },
                workerUnavailableMessage: "助手未运行",
                ensureWorker: { true },
                fetch: {
                    fetchCalls += 1
                    return try await withCheckedThrowingContinuation { gate.continuation = $0 }
                }
            )
        }

        for _ in 0..<2000 where gate.continuation == nil {
            await Task.yield()
        }
        XCTAssertNotNil(gate.continuation)

        // 在途期间自动触发全部防重跳过
        for trigger: ConsoleAccountsRefreshTrigger in [.initialLoad, .windowActivated, .pageEntered, .scheduled] {
            let outcome = await coordinator.run(
                trigger: trigger,
                now: { base.addingTimeInterval(1) },
                workerUnavailableMessage: "助手未运行",
                ensureWorker: { true },
                fetch: {
                    fetchCalls += 1
                    return []
                }
            )
            XCTAssertEqual(outcome, .skipped(.inFlight), "触发 \(trigger) 应当防重跳过")
        }
        XCTAssertEqual(fetchCalls, 1)

        gate.continuation?.resume(returning: [sampleAccount("a1")])
        let result = await firstTask.value
        guard case .succeeded = result else {
            return XCTFail("首个请求应正常落地：\(result)")
        }
    }

    func testOlderSlowRefreshCannotOverwriteNewerManualResult() async {
        let base = Date(timeIntervalSince1970: 1_795_400_000)
        var current = base
        let coordinator = ConsoleAccountsRefreshCoordinator(ttl: 600)
        let slowGate = AccountsGate()

        // 1) 启动一个慢自动刷新（generation = 1）
        let slowTask = Task { @MainActor in
            await coordinator.run(
                trigger: .initialLoad,
                now: { current },
                workerUnavailableMessage: "助手未运行",
                ensureWorker: { true },
                fetch: {
                    try await withCheckedThrowingContinuation { slowGate.continuation = $0 }
                }
            )
        }
        for _ in 0..<2000 where slowGate.continuation == nil {
            await Task.yield()
        }
        XCTAssertNotNil(slowGate.continuation)

        // 2) 用户点击手动刷新（generation = 2）并抢先返回新数据
        let manualAt = base.addingTimeInterval(2)
        current = manualAt
        let manualOutcome = await coordinator.run(
            trigger: .manual,
            now: { current },
            workerUnavailableMessage: "助手未运行",
            ensureWorker: { true },
            fetch: { [self.sampleAccount("fresh", nickname: "新快照")] }
        )
        XCTAssertEqual(manualOutcome, .succeeded(accounts: [sampleAccount("fresh", nickname: "新快照")], at: manualAt))
        XCTAssertEqual(coordinator.state.lastSuccessAt, manualAt)
        XCTAssertFalse(coordinator.state.isLoading)

        // 3) 此时旧慢请求才返回旧数据（哪怕时间更晚），必须被丢弃，不能覆盖 lastSuccessAt 或错误状态
        current = base.addingTimeInterval(10)
        slowGate.continuation?.resume(returning: [sampleAccount("stale", nickname: "旧快照")])
        let slowOutcome = await slowTask.value
        XCTAssertEqual(slowOutcome, .superseded, "旧请求返回后不得覆盖新结果")
        XCTAssertEqual(coordinator.state.lastSuccessAt, manualAt, "旧请求不得改写成功时间")
        XCTAssertNil(coordinator.state.lastError)
        XCTAssertFalse(coordinator.state.isLoading)
    }

    func testFailureBackoffEscalatesAndManualBypassesBackoff() async {
        XCTAssertEqual(ConsoleAccountsRefreshCoordinator.failureBackoff(failures: 0), 0)
        XCTAssertEqual(ConsoleAccountsRefreshCoordinator.failureBackoff(failures: 1), 30)
        XCTAssertEqual(ConsoleAccountsRefreshCoordinator.failureBackoff(failures: 2), 60)
        XCTAssertEqual(ConsoleAccountsRefreshCoordinator.failureBackoff(failures: 3), 120)
        XCTAssertEqual(ConsoleAccountsRefreshCoordinator.failureBackoff(failures: 4), 240)
        XCTAssertEqual(ConsoleAccountsRefreshCoordinator.failureBackoff(failures: 5), 480)
        XCTAssertEqual(ConsoleAccountsRefreshCoordinator.failureBackoff(failures: 6), 600)

        let base = Date(timeIntervalSince1970: 1_795_400_000)
        var current = base
        let coordinator = ConsoleAccountsRefreshCoordinator(ttl: 600, baseFailureBackoff: 30, maxFailureBackoff: 600)

        // 第 1 次失败 -> 退避 30s
        _ = await coordinator.run(
            trigger: .initialLoad,
            now: { current },
            workerUnavailableMessage: "助手未运行",
            ensureWorker: { false },
            fetch: { [] }
        )
        XCTAssertEqual(coordinator.state.consecutiveFailures, 1)

        // 10s 内自动触发被退避挡住
        current = base.addingTimeInterval(10)
        let skipped = await coordinator.run(
            trigger: .windowActivated,
            now: { current },
            workerUnavailableMessage: "助手未运行",
            ensureWorker: { true },
            fetch: { [] }
        )
        XCTAssertEqual(skipped, .skipped(.backoff(remaining: 20)))

        // 满 30s 允许自动重试；再失败 -> 退避升为 60s
        current = base.addingTimeInterval(30)
        _ = await coordinator.run(
            trigger: .scheduled,
            now: { current },
            workerUnavailableMessage: "助手未运行",
            ensureWorker: { false },
            fetch: { [] }
        )
        XCTAssertEqual(coordinator.state.consecutiveFailures, 2)
        XCTAssertEqual(coordinator.nextAutoRefreshDelay(now: current), 60)

        // 退避期间用户手动刷新立刻放行，成功后重置退避计数
        current = base.addingTimeInterval(40)
        let manual = await coordinator.run(
            trigger: .manual,
            now: { current },
            workerUnavailableMessage: "助手未运行",
            ensureWorker: { true },
            fetch: { [self.sampleAccount("ok")] }
        )
        guard case .succeeded = manual else {
            return XCTFail("手动刷新必须绕过失败退避：\(manual)")
        }
        XCTAssertEqual(coordinator.state.consecutiveFailures, 0)
        XCTAssertEqual(coordinator.nextAutoRefreshDelay(now: current), 600)
    }

    // MARK: - 3. 前台持续停留 10 分钟定时调度 + 后台 / 切页 / 关闭暂停

    func testForegroundSchedulerFiresEveryTenMinutesWhileStayingOnOverviewAndPausesOnBackgroundOrOtherPages() async {
        let base = Date(timeIntervalSince1970: 1_795_400_000)
        let clock = TestClock(now: base)
        let coordinator = ConsoleAccountsRefreshCoordinator(ttl: 600)
        let sleepRecorder = SleepRecorder(clock: clock)

        // 冷启动先完成一次加载
        _ = await coordinator.run(
            trigger: .initialLoad,
            now: { clock.now },
            workerUnavailableMessage: "助手未运行",
            ensureWorker: { true },
            fetch: { [self.sampleAccount("init")] }
        )

        var tickCount = 0
        let scheduler = ConsoleAccountsAutoRefreshScheduler(
            sleep: { seconds in try await sleepRecorder.sleep(seconds) },
            now: { clock.now }
        )

        let nextDelay: (Date) -> TimeInterval? = { coordinator.nextAutoRefreshDelay(now: $0) }
        let onTick: () async -> Void = {
            let outcome = await coordinator.run(
                trigger: .scheduled,
                now: { clock.now },
                workerUnavailableMessage: "助手未运行",
                ensureWorker: { true },
                fetch: {
                    tickCount += 1
                    return [self.sampleAccount("tick-\(tickCount)")]
                }
            )
            if case .succeeded = outcome {} else {
                XCTFail("定时刷新应当成功：\(outcome)")
            }
        }

        // 前台停留在总览页 -> 应排期等待 600s
        scheduler.update(isForeground: true, activePage: .overview, nextDelay: nextDelay, onTick: onTick)
        XCTAssertTrue(scheduler.isScheduled)
        let delay1 = await sleepRecorder.waitForNextSleep()
        XCTAssertEqual(delay1, 600)

        // 持续停留 10 分钟，不切页不切窗 -> 自动触发第 1 次定时刷新
        sleepRecorder.advanceAndResume(600)
        let delay2 = await sleepRecorder.waitForNextSleep()
        XCTAssertEqual(tickCount, 1, "前台持续停留满 10 分钟必须触发第 1 次自动只读刷新")
        XCTAssertEqual(delay2, 600)

        // 再持续停留 10 分钟 -> 触发第 2 次
        sleepRecorder.advanceAndResume(600)
        _ = await sleepRecorder.waitForNextSleep()
        XCTAssertEqual(tickCount, 2, "继续停留满 20 分钟必须触发第 2 次自动只读刷新")

        // 切到额度页 -> 立刻暂停调度
        scheduler.update(isForeground: true, activePage: .quota, nextDelay: nextDelay, onTick: onTick)
        XCTAssertFalse(scheduler.isScheduled)
        XCTAssertFalse(scheduler.isEligible)

        // 切回 AI 账号页（已过 200s）-> 仅等待剩余 400s
        clock.now = clock.now.addingTimeInterval(200)
        scheduler.update(isForeground: true, activePage: .accounts, nextDelay: nextDelay, onTick: onTick)
        XCTAssertTrue(scheduler.isScheduled)
        let remainingDelay = await sleepRecorder.waitForNextSleep()
        XCTAssertEqual(remainingDelay, 400)

        // 切到后台 -> 立刻暂停调度
        scheduler.update(isForeground: false, activePage: .accounts, nextDelay: nextDelay, onTick: onTick)
        XCTAssertFalse(scheduler.isScheduled)

        // 关闭窗口 -> 停止调度
        scheduler.stop()
        XCTAssertFalse(scheduler.isScheduled)
    }

    // MARK: - 4. 页头「账号更新于」文案只代表账号快照最后成功读取时间

    func testHeaderStatusCopyOnlyReflectsLastSuccessfulAccountSnapshot() {
        let base = Date(timeIntervalSince1970: 1_795_400_000)
        let fmt: (Date) -> String = { _ in "21:30" }

        // 非账号相关页不显示
        XCTAssertNil(ConsoleAccountsHeaderStatus.make(
            page: .quota, lastSuccessAt: base, error: nil, now: base, formatTime: fmt
        ))
        XCTAssertNil(ConsoleAccountsHeaderStatus.make(
            page: .checkin, lastSuccessAt: base, error: nil, now: base, formatTime: fmt
        ))

        // 正常新鲜快照：明确写「账号更新于」
        let fresh = ConsoleAccountsHeaderStatus.make(
            page: .overview, lastSuccessAt: base, error: nil, now: base.addingTimeInterval(60), formatTime: fmt
        )
        XCTAssertEqual(fresh?.text, "账号更新于 21:30")
        XCTAssertEqual(fresh?.tone, .neutral)
        XCTAssertEqual(fresh?.isStale, false)

        // 刷新失败且保留旧快照：时间不推进，明确标「已过期」
        let staleAfterError = ConsoleAccountsHeaderStatus.make(
            page: .accounts,
            lastSuccessAt: base,
            error: "账号助手未就绪",
            now: base.addingTimeInterval(60),
            formatTime: fmt
        )
        XCTAssertEqual(staleAfterError?.text, "账号更新于 21:30 · 已过期")
        XCTAssertEqual(staleAfterError?.tone, .warning)
        XCTAssertEqual(staleAfterError?.isStale, true)
        XCTAssertTrue(staleAfterError?.help.contains("账号助手未就绪") == true)

        // 从未成功读取且失败：绝不能显示假的「更新于」时间
        let neverSucceeded = ConsoleAccountsHeaderStatus.make(
            page: .overview,
            lastSuccessAt: nil,
            error: "助手未运行",
            now: base,
            formatTime: fmt
        )
        XCTAssertEqual(neverSucceeded?.text, "账号快照未读取")
        XCTAssertEqual(neverSucceeded?.tone, .warning)
        XCTAssertEqual(neverSucceeded?.isStale, true)
    }
}

private enum RefreshTestError: LocalizedError {
    case rpcTimeout
    var errorDescription: String? { "RPC 读取超时" }
}

@MainActor
private final class AccountsGate {
    var continuation: CheckedContinuation<[ConsoleAccount], Error>?
}

@MainActor
private final class TestClock {
    var now: Date
    init(now: Date) { self.now = now }
}

@MainActor
private final class SleepRecorder {
    private let clock: TestClock
    private var activeID: UUID?
    private var recordedDelays: [TimeInterval] = []
    private var pendingSleep: CheckedContinuation<Void, Error>?
    private var sleepWaiter: CheckedContinuation<TimeInterval, Never>?

    init(clock: TestClock) {
        self.clock = clock
    }

    func sleep(_ seconds: TimeInterval) async throws {
        let id = UUID()
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
                Task { @MainActor in
                    if Task.isCancelled {
                        continuation.resume(throwing: CancellationError())
                        return
                    }
                    self.activeID = id
                    self.pendingSleep = continuation
                    if let waiter = self.sleepWaiter {
                        self.sleepWaiter = nil
                        waiter.resume(returning: seconds)
                    } else {
                        self.recordedDelays.append(seconds)
                    }
                }
            }
        } onCancel: {
            Task { @MainActor in
                guard self.activeID == id else { return }
                self.activeID = nil
                self.pendingSleep?.resume(throwing: CancellationError())
                self.pendingSleep = nil
            }
        }
    }

    func waitForNextSleep() async -> TimeInterval {
        if !recordedDelays.isEmpty {
            return recordedDelays.removeFirst()
        }
        return await withCheckedContinuation { continuation in
            self.sleepWaiter = continuation
        }
    }

    func advanceAndResume(_ seconds: TimeInterval) {
        clock.now = clock.now.addingTimeInterval(seconds)
        let cont = pendingSleep
        activeID = nil
        pendingSleep = nil
        cont?.resume()
    }
}
