import Foundation

/// `SettingsWorkerPhase` 的视图侧桥接：只有这里认识 `AccountWorkerManager.State`。
///
/// 纯模型文件（`Models/SettingsCenter.swift`）不引用 Manager，才能在
/// AICCCore 测试目标里编译与测试。
extension SettingsWorkerPhase {
    static func resolve(_ state: AccountWorkerManager.State) -> SettingsWorkerPhase {
        switch state {
        case .stopped:
            return .stopped
        case .starting:
            return .starting
        case .running(let health):
            if health.isProtocolCompatible {
                // 设置页只留"版本 + 账号数"；数据库/平台登录状态属于账号页与诊断日志。
                return .running("v\(health.workerVersion) · \(health.accountCount) 个账号")
            }
            return .incompatible("助手协议 v\(health.protocolVersion)，App 支持 v\(AccountWorkerProtocol.version)。请更新 AICC 或重新安装完整安装包。")
        case .failed(let message):
            // 诊断要留关键细节，但不能把整段进程日志倒进设置页：最多三行。
            let lines = message.split(separator: "\n").prefix(3).map(String.init)
            let detail = lines.joined(separator: "\n")
            if detail.contains("协议版本不匹配") { return .incompatible(detail) }
            return .failed(detail.isEmpty ? "启动失败" : detail)
        }
    }
}
