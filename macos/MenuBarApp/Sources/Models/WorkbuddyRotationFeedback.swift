import Foundation

/// 轮换操作的结果文案。
///
/// 抽成纯函数有两个原因：
///
/// 1. **不能把"提交了"说成"开始了"**。后端会返回 `already_ran` / `skipped` /
///    `busy` 这些"什么也没发生"的状态，界面必须分开说。
/// 2. 这些分支要能被单测钉住 —— 它们直接决定用户以为发生了什么。
///
/// 这里只做"状态 → 一句话 + 是否算失败"，不碰任何 IO。
struct RotationFeedback: Equatable {
    let text: String
    let isFailure: Bool

    init(_ text: String, isFailure: Bool = false) {
        self.text = text
        self.isFailure = isFailure
    }
}

enum RotationFeedbackText {
    /// 「立即执行一轮」的返回。
    static func runNow(status: String, detail: String?) -> RotationFeedback {
        switch status {
        case "busy":
            return .init("另一项轮换操作正在执行，请稍后再试。")
        case "already_ran":
            return .init(
                "今天已经执行过一轮了（自动和手动都算一天一次），可以在「执行记录」里看结果。"
            )
        case "skipped":
            return .init("这一轮没有执行：\(detail ?? "见执行记录")", isFailure: true)
        case "invalid_plan":
            return .init("计划配置不完整：\(detail ?? "请先保存可用的计划")", isFailure: true)
        case "not_configured":
            return .init("还没有选择参与轮换的账号。", isFailure: true)
        case "disabled":
            return .init("自动轮换是关闭的，手动一轮没有被执行。", isFailure: true)
        case "prechecking":
            return .init("已开始这一轮：正在检查凭据。结束时会恢复到最初的账号。")
        case "switching":
            return .init("已开始这一轮：正在切换账号。结束时会恢复到最初的账号。")
        case "dwelling":
            return .init("已开始这一轮：已经切到第一个账号，正在按停留时长往下走。")
        case "restoring":
            return .init("已开始收尾：正在恢复这一轮最初的账号。")
        case "completed":
            return .init("这一轮已经结束，并已恢复原账号。")
        default:
            return .init("已提交，当前状态：\(stateText(status))。")
        }
    }

    /// 「暂停并恢复原账号」的返回。
    ///
    /// `busy` 必须单独说成"尚未暂停"：拿不到执行权就说"正在收尾"是骗人的。
    static func stop(status: String, restored: Bool?, detail: String?) -> RotationFeedback {
        switch status {
        case "busy":
            return .init("尚未暂停：另一个轮换操作正在执行，请稍后重试。", isFailure: true)
        case "nothing_to_stop":
            return .init("当前没有正在执行的一轮。")
        default:
            switch restored {
            case .some(true):
                return .init("已暂停，并恢复了原账号。")
            case .some(false):
                return .init(
                    detail ?? "已请求暂停，但原账号没能自动恢复；请点「恢复最初账号…」。",
                    isFailure: true
                )
            case .none:
                return .init("已暂停。")
            }
        }
    }

    /// 开关的返回。关闭是两件事：停止后续、把账号恢复回来。
    static func toggle(
        enabled: Bool,
        stopStatus: String?,
        restored: Bool?
    ) -> RotationFeedback {
        if enabled {
            return .init("已打开。计划只在 AICC 运行时执行；开始前会先检查凭据，凭据不对就跳过那个账号。")
        }
        switch (stopStatus, restored) {
        case ("busy", _):
            return .init("计划已关闭；当前还有一个轮换操作在执行，结束后会自动恢复原账号。")
        case (_, .some(true)):
            return .init("已关闭，并且已恢复原账号。")
        case (_, .some(false)):
            return .init("已关闭，但原账号没能自动恢复；请点「恢复最初账号…」并确认。", isFailure: true)
        default:
            return .init("已关闭，不会再自动切换客户端。")
        }
    }

    /// 表单门禁：有未保存的修改时，预演与立即执行都必须禁用。
    static func canPreview(dirty: Bool, busy: Bool) -> Bool { !dirty && !busy }
    static func canRunNow(dirty: Bool, busy: Bool) -> Bool { !dirty && !busy }

    /// 「保存后开启」：保存失败就不能继续开启，否则等于用旧配置打开计划。
    static func shouldApplyEnableAfterSave(saveSucceeded: Bool) -> Bool { saveSucceeded }

    static func stateText(_ state: String) -> String {
        switch state {
        case "scheduled": return "已排定"
        case "prechecking": return "正在检查凭据"
        case "switching": return "正在切换账号"
        case "dwelling": return "正在停留"
        case "restoring": return "正在恢复原账号"
        case "completed": return "已完成"
        case "skipped": return "已跳过"
        case "failed": return "失败"
        case "recovery_required": return "需要你确认"
        default: return state
        }
    }
}
