import Foundation

/// 总览页对「轮换恢复异常」的可见性状态。
///
/// 三种情况都不能被当成"一切正常"：
/// - 还没读到（冷启动/加载中）；
/// - 读取失败（异常与否未知）；
/// - 已读到且 `attentionRun` 存在（必须有人处理）。
///
/// 这层状态机放在 Models 里，视图只负责渲染；冷启动只有
/// `lastRun=recovery_required` 时也能被确定性测试覆盖。
enum ConsoleRotationAttentionState: Equatable {
    /// 还没有可用的轮换状态（首次加载/刷新中）。
    case loading
    /// 已读到状态，且存在需要人处理的轮换轮次。
    case needsAction(WorkbuddyRotationRun)
    /// 读取失败：不能声明正常，也不能伪造成功。
    case unavailable(String)
    /// 已读到状态，没有恢复异常。
    case clear

    init(status: WorkbuddyRotationStatus?, error: String?) {
        if let attention = status?.attentionRun {
            self = .needsAction(attention)
        } else if let error, !error.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            // 即使手里有旧状态，本次读取失败也不能当成"已确认正常"。
            self = .unavailable(error)
        } else if status != nil {
            self = .clear
        } else {
            self = .loading
        }
    }

    var needsActionRun: WorkbuddyRotationRun? {
        if case .needsAction(let run) = self { return run }
        return nil
    }

    /// 状态未知：加载中或读取失败。
    var isUnknown: Bool {
        switch self {
        case .loading, .unavailable: return true
        case .needsAction, .clear: return false
        }
    }

    /// 计入"需要处理"的数量：恢复异常与未知状态各算一项，不让首屏显示 0。
    var countContribution: Int {
        switch self {
        case .needsAction, .unavailable: return 1
        case .loading, .clear: return 0
        }
    }

    /// 待处理列表里的一行标题；`nil` = 不需要出现。
    var pendingTitle: String? {
        switch self {
        case .needsAction: return "WorkBuddy 轮换未恢复"
        case .unavailable: return "轮换状态无法确认"
        case .loading, .clear: return nil
        }
    }

    /// 待处理列表里的说明。对未知状态只说下一步，不把底层错误原文铺到首屏。
    var pendingDetail: String? {
        switch self {
        case .needsAction(let run): return run.restoreText
        case .unavailable: return "状态读取失败，请打开轮换面板确认；不会自动恢复"
        case .loading, .clear: return nil
        }
    }
}

/// 冷启动加载策略：轮换状态只在"还没有、也不在读取中"时补一次。
///
/// 单独抽出来是为了让"首次加载会读轮换状态"成为确定性测试，
/// 而不是只能靠人肉打开 App 观察。
enum ConsoleColdStartLoadPolicy {
    static func needsRotation(status: WorkbuddyRotationStatus?, loading: Bool) -> Bool {
        status == nil && !loading
    }
}

/// 总览首屏的"需要处理"汇总。
///
/// 把原先散在视图里的计数与标题判断收拢成一个可测试的值；
/// 视图不再自己决定"未知算不算正常"。
struct ConsoleOverviewAttention: Equatable {
    let fleetIssues: Int
    let failureCount: Int
    let accountsError: Bool
    let qoderError: Bool
    let rotation: ConsoleRotationAttentionState

    init(
        fleetIssues: Int,
        failureCount: Int,
        accountsError: Bool,
        qoderError: Bool,
        rotation: ConsoleRotationAttentionState
    ) {
        self.fleetIssues = fleetIssues
        self.failureCount = failureCount
        self.accountsError = accountsError
        self.qoderError = qoderError
        self.rotation = rotation
    }

    var count: Int {
        fleetIssues + failureCount
            + (accountsError ? 1 : 0)
            + (qoderError ? 1 : 0)
            + rotation.countContribution
    }

    var statusUnknown: Bool { rotation.isUnknown }

    /// 首屏大标题。只有"已确认没有异常"才允许出现"一切正常"。
    var headline: String {
        if count > 0 { return "\(count) 项需要处理" }
        if statusUnknown { return "状态待确认" }
        return "一切正常"
    }
}
