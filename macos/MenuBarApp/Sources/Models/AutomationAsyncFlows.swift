import Foundation

// MARK: - 轮换状态读取

/// 一次轮换状态读取的结果。
///
/// `silent` 只决定"要不要打扰用户"，不能决定"要不要记录失败"：
/// Worker 起不来时也必须有 error，界面才能进入 stale/failed 而不是继续绿色。
struct RotationRefreshOutcome: Equatable {
    let status: WorkbuddyRotationStatus?
    let error: String?

    static func describe(_ error: Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
    }
}

@MainActor
enum RotationRefreshFlow {
    static func run(
        cached: WorkbuddyRotationStatus?,
        workerMessage: String,
        ensureWorker: () async -> Bool,
        fetch: () async throws -> WorkbuddyRotationStatus
    ) async -> RotationRefreshOutcome {
        guard await ensureWorker() else {
            // 保留最后成功快照，但一定带上"这次没读到"的事实。
            return RotationRefreshOutcome(status: cached, error: workerMessage)
        }
        do {
            return RotationRefreshOutcome(status: try await fetch(), error: nil)
        } catch {
            return RotationRefreshOutcome(status: cached, error: RotationRefreshOutcome.describe(error))
        }
    }
}

// MARK: - Qoder Credits 自动领取状态

/// 一次自动领取状态的读取结果。
///
/// 读失败**不能**清空上次成功快照：那会把"2 个账号已开启"变成
/// "上次：暂无支持账号"，等于编造历史。
struct QoderCreditsRefreshOutcome: Equatable {
    let items: [QoderCreditsAutoItem]
    let loaded: Bool
    let error: String?
}

@MainActor
enum QoderCreditsRefreshFlow {
    static func run(
        cached: [QoderCreditsAutoItem],
        loaded: Bool,
        fetch: () async throws -> [QoderCreditsAutoItem]
    ) async -> QoderCreditsRefreshOutcome {
        do {
            return QoderCreditsRefreshOutcome(items: try await fetch(), loaded: true, error: nil)
        } catch {
            return QoderCreditsRefreshOutcome(
                items: cached,
                loaded: loaded,
                error: RotationRefreshOutcome.describe(error)
            )
        }
    }
}

// MARK: - 单账号签到复查

/// 一次单账号签到复查的结果。
///
/// 只有真正读到这一行才算成功；失败时**保留**上一份已确认状态，
/// 由界面把原因单独显示成异常，不能把"没读成"当成最新状态。
enum CheckinRecheckOutcome: Equatable {
    case refreshed(CheckinAccountRow)
    case failed(reason: String)
}

/// `ConsoleStore.recheckCheckin` 实际调用的协调逻辑：只读一次 `checkin.status`，
/// 不碰 `checkin.run`、不写任何记录。
@MainActor
enum CheckinRecheckFlow {
    static func run(
        accountId: String,
        workerMessage: String,
        ensureWorker: () async -> Bool,
        fetch: () async throws -> CheckinFleetStatus
    ) async -> CheckinRecheckOutcome {
        guard await ensureWorker() else {
            return .failed(reason: workerMessage)
        }
        do {
            let fleet = try await fetch()
            guard let fresh = fleet.row(accountId) else {
                return .failed(reason: "服务端没有返回这个账号的今日状态")
            }
            return .refreshed(fresh)
        } catch {
            return .failed(reason: RotationRefreshOutcome.describe(error))
        }
    }
}

// MARK: - 轮换预演

/// 一次预演请求的身份：发起时的序号、计划摘要与计划代次。
///
/// 服务端 `dry_run` 读的是**已保存**计划，请求往返期间计划可能被改掉。
/// 只把结果按"返回时"的计划贴标签会把 A 的结果算到 B 头上，所以身份在
/// 发起时就固定下来。
struct RotationPreviewTicket: Equatable {
    let sequence: Int
    let planKey: String
    let generation: Int
}

enum RotationPreviewResult: Equatable {
    case applied(WorkbuddyRotationDryRun, planKey: String)
    /// 已经有更新的预演请求：旧结果直接丢弃，不能覆盖新结果。
    case superseded
    /// 请求期间已保存计划变了（A → B，甚至 A → B → A）。
    case planChanged
    case failed(String)
}

/// 预演请求的序号发放器。界面只通过它判断"谁是最新"。
@MainActor
final class RotationPreviewCoordinator {
    private(set) var latestSequence = 0

    func begin(planKey: String?, generation: Int) -> RotationPreviewTicket? {
        guard let planKey else { return nil }
        latestSequence += 1
        return RotationPreviewTicket(sequence: latestSequence, planKey: planKey, generation: generation)
    }

    func isLatest(_ ticket: RotationPreviewTicket) -> Bool {
        ticket.sequence == latestSequence
    }

    /// 计划在请求期间变了：让这次请求（以及更早的）都不能再落库。
    func invalidate() {
        latestSequence += 1
    }
}

@MainActor
enum RotationPreviewFlow {
    static func run(
        coordinator: RotationPreviewCoordinator,
        planKey: String?,
        generation: Int,
        currentGeneration: () -> Int,
        fetch: () async throws -> WorkbuddyRotationDryRun
    ) async -> RotationPreviewResult {
        guard let ticket = coordinator.begin(planKey: planKey, generation: generation) else {
            return .failed("还没有可预演的计划。")
        }
        do {
            let dryRun = try await fetch()
            // 乱序返回：较新的请求已经落地，旧结果不能覆盖它。
            guard coordinator.isLatest(ticket) else { return .superseded }
            // 计划在请求往返期间改过：服务端读到的可能已是另一份计划。
            guard ticket.generation == currentGeneration() else {
                coordinator.invalidate()
                return .planChanged
            }
            return .applied(dryRun, planKey: ticket.planKey)
        } catch {
            guard coordinator.isLatest(ticket) else { return .superseded }
            return .failed(RotationRefreshOutcome.describe(error))
        }
    }
}
