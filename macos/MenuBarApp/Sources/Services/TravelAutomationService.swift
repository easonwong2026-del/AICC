import Foundation

/// 让「自动旅行」在 App 活着的时候真的有人在跑。
///
/// 事实先说清楚（界面和文档都不许含糊）：
/// - 调度器在 `aicc-account-worker` 里，而它是 `AICC.app` 的子进程；
/// - 所以 **AICC.app 退出 = 自动旅行停止**，不存在"退出后还在后台跑"；
/// - 这个类只做两件事：启动时按需拉起助手、助手挂掉时重启它。
///
/// 为什么需要它：助手原本是"点开控制台才启动"的懒加载进程。用户设了自动旅行
/// 之后如果重启 App 却不打开控制台，就没人调 tick 了 —— 那才是真正的"虚称在跑"。
@MainActor
final class TravelAutomationService {
    static let shared = TravelAutomationService()

    /// 上次退出前是否有账号开着自动旅行。只存布尔值，不存账号信息。
    private static let defaultsKey = "aicc.travelAutoEnabled"

    private var watchdog: Task<Void, Never>?

    private init() {}

    /// 记录「是否需要开机即恢复自动旅行」。控制台每次拿到快照都会调它。
    static func remember(_ enabled: Bool) {
        UserDefaults.standard.set(enabled, forKey: defaultsKey)
    }

    static var previouslyEnabled: Bool {
        UserDefaults.standard.bool(forKey: defaultsKey)
    }

    /// App 启动时调用：只有上次确实开着自动旅行才把助手拉起来。
    func startIfNeeded(worker: AccountWorkerManager) {
        guard Self.previouslyEnabled else { return }
        guard watchdog == nil else { return }
        watchdog = Task { [weak worker] in
            while !Task.isCancelled {
                guard let worker else { return }
                if case .running = worker.state {
                    // 助手活着就只是守着；它死了才重启。
                } else {
                    await worker.refresh()
                }
                // 助手自己会每 60 秒 tick 一次，这里只需要低频看护。
                try? await Task.sleep(nanoseconds: 120 * 1_000_000_000)
            }
        }
    }

    /// 所有账号都关掉自动旅行之后调用，别留一个空转的看护任务。
    func stopIfIdle() {
        guard !Self.previouslyEnabled else { return }
        watchdog?.cancel()
        watchdog = nil
    }

    deinit {
        watchdog?.cancel()
    }
}
