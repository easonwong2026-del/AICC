import Foundation

/// Qoder 的一个账号 Profile。
///
/// **Qoder 客户端忽略 `--user-data-dir`**（M0 实测），一个 OS 用户只有一份
/// `userData`，所以「Profile」在这里的含义是**一份认证快照**，不是独立目录：
///
/// - `liveDir`：客户端真实数据目录（同版本共享）
/// - `snapshotDir`：AICC 自己管的快照目录（每个 Profile 独占）
///
/// ⚠️ 快照只包含认证文件组，**不等于完整 Profile 隔离**：
/// `main.sqlite` / `Local State` / Cookies 里仍有账号绑定数据。
/// 所以本阶段 `switchEnabled` 恒为 false，`open` / `activate` 一律拒绝。
///
/// 模型永远不含令牌，只有指纹（sha256 前 8 位）与到期时间。
struct QoderProfile: Identifiable, Equatable {
    let id: String
    let edition: String
    let displayName: String
    let accountUid: String?
    let email: String?
    let appBundleId: String?
    let appPath: String?
    let clientVersion: String?
    let liveDir: String
    let snapshotDir: String
    let authFile: String?
    let authFingerprint: String?
    let authStatus: String
    let expiresAt: String?
    let managed: Bool
    let lastOpenedAt: String?
    /// 快照里有没有认证文件（上一次保存被中断时为 false）。
    let snapshotReady: Bool
    /// 上一次保存快照没有完成，快照可能只有半套文件。
    let snapshotIncomplete: Bool
    /// live 当前登录的是不是这个 Profile（按文件指纹比对）。
    let active: Bool
    /// 这个版本能不能做真实快照切换（默认关闭，原因见 switchBlockedReason）。
    let switchEnabled: Bool
    let switchBlockedReason: String?
    let runtime: Runtime

    struct Runtime: Equatable {
        let state: String
        let pid: Int?

        var isRunning: Bool { state == "running" }
        /// 进程活着但无法证明是我们这个 Profile（或读不到命令行）。
        var isUnknown: Bool { state == "unknown" }
    }

    init?(json: [String: Any]) {
        guard let id = json["profileId"] as? String,
              let edition = json["edition"] as? String,
              let snapshotDir = json["snapshotDir"] as? String else { return nil }
        self.id = id
        self.edition = edition
        self.displayName = json["displayName"] as? String ?? edition
        self.accountUid = json["accountUid"] as? String
        self.email = json["email"] as? String
        self.appBundleId = json["appBundleId"] as? String
        self.appPath = json["appPath"] as? String
        self.clientVersion = json["clientVersion"] as? String
        self.liveDir = json["liveDir"] as? String ?? ""
        self.snapshotDir = snapshotDir
        self.authFile = json["authFile"] as? String
        self.authFingerprint = json["authFingerprint"] as? String
        self.authStatus = json["authStatus"] as? String ?? "unknown"
        self.expiresAt = json["expiresAt"] as? String
        self.managed = json["managed"] as? Bool ?? true
        self.lastOpenedAt = json["lastOpenedAt"] as? String
        self.snapshotReady = json["snapshotReady"] as? Bool ?? false
        self.snapshotIncomplete = json["snapshotIncomplete"] as? Bool ?? false
        self.active = json["active"] as? Bool ?? false
        self.switchEnabled = json["switchEnabled"] as? Bool ?? false
        self.switchBlockedReason = json["switchBlockedReason"] as? String
        let runtimeJSON = json["runtime"] as? [String: Any] ?? [:]
        self.runtime = Runtime(
            state: runtimeJSON["state"] as? String ?? "stopped",
            pid: runtimeJSON["pid"] as? Int
        )
    }

    var editionName: String {
        switch edition {
        case "qoder_cn": return "Qoder CN"
        case "qoder_global": return "Qoder International"
        default: return edition
        }
    }

    var symbolName: String {
        edition == "qoder_global" ? "globe" : "chevron.left.forwardslash.chevron.right"
    }

    /// 可靠识别到的身份：UID 优先，其次邮箱。都没有就返回 nil，**不编造**。
    var identityText: String? {
        if let uid = accountUid, !uid.isEmpty { return uid }
        if let email = email, !email.isEmpty { return email }
        return nil
    }

    var identityLabel: String {
        if accountUid != nil { return "UID" }
        if email != nil { return "邮箱" }
        return "身份"
    }

    var authLabel: String {
        if !snapshotReady && authStatus == "missing" { return "未保存登录" }
        switch authStatus {
        case "valid": return "已保存"
        case "expired": return "已过期"
        case "invalid": return "认证损坏"
        case "denied": return "授权被拒"
        case "missing": return "未登录"
        default: return active ? "当前账号" : "待检查"
        }
    }

    var authTone: ConsoleAccount.AuthTone {
        switch authStatus {
        case "valid": return .ok
        case "expired": return .warning
        case "invalid", "denied": return .error
        default: return snapshotReady ? .warning : .neutral
        }
    }

    var runtimeLabel: String {
        if runtime.isRunning { return active ? "运行中（当前）" : "运行中" }
        if runtime.isUnknown { return "状态未知" }
        return active ? "当前账号" : "未运行"
    }

    var runtimeTone: ConsoleAccount.AuthTone {
        if runtime.isRunning { return .ok }
        if runtime.isUnknown { return .warning }
        return active ? .ok : .neutral
    }

    var fingerprintShort: String {
        guard let authFingerprint, !authFingerprint.isEmpty else { return "—" }
        return authFingerprint
    }

    /// 主按钮文案。本阶段切换恒为关闭，所以恒为"切换未开放"并禁用。
    var primaryActionTitle: String {
        if !switchEnabled { return "切换未开放" }
        if runtime.isRunning && active { return "已在此账号" }
        if !snapshotReady { return "登录并保存" }
        return active ? "打开" : "切换并打开"
    }

    var searchText: String {
        [displayName, editionName, identityText ?? "", liveDir]
            .joined(separator: " ")
            .lowercased()
    }
}

/// `qoder.editions.detect` 的单条结果。
struct QoderEditionInfo: Identifiable, Equatable {
    let edition: String
    let displayName: String
    let bundleId: String
    let installed: Bool
    let appPath: String?
    let clientVersion: String?
    let dataDirCount: Int
    let parallelInstances: String
    let switchMode: String
    let switchEnabled: Bool
    let switchBlockedReason: String?
    let verifiedOnThisMachine: Bool
    let loggedIn: Bool
    let loggedInEmail: String?

    var id: String { edition }

    init?(json: [String: Any]) {
        guard let edition = json["edition"] as? String else { return nil }
        self.edition = edition
        self.displayName = json["displayName"] as? String ?? edition
        self.bundleId = json["bundleId"] as? String ?? ""
        self.installed = json["installed"] as? Bool ?? false
        self.appPath = json["appPath"] as? String
        self.clientVersion = json["clientVersion"] as? String
        self.dataDirCount = (json["dataDirs"] as? [Any])?.count ?? 0
        self.parallelInstances = json["parallelInstances"] as? String ?? "unknown"
        self.switchMode = json["switchMode"] as? String ?? "snapshot"
        self.switchEnabled = json["switchEnabled"] as? Bool ?? false
        self.switchBlockedReason = json["switchBlockedReason"] as? String
        self.verifiedOnThisMachine = json["verifiedOnThisMachine"] as? Bool ?? false
        self.loggedIn = json["loggedIn"] as? Bool ?? false
        self.loggedInEmail = json["loggedInEmail"] as? String
    }

    var installSummary: String {
        if installed {
            let version = clientVersion.map { " v\($0)" } ?? ""
            return "已安装\(version)"
        }
        return "未安装"
    }

    /// 并行能力的人话口径：`no` = 实测不支持，`unknown` = 未复验。
    var parallelLabel: String {
        switch parallelInstances {
        case "no": return "实测不支持并行"
        case "yes": return "允许并行"
        default: return "并行能力未复验"
        }
    }

    /// 客户端当前登录的账号（来自客户端自己写的状态文件，不是 AICC 推断）。
    var currentAccountHint: String? {
        guard loggedIn else { return nil }
        return loggedInEmail ?? "已登录"
    }
}

enum QoderEdition: String, CaseIterable, Identifiable {
    case cn = "qoder_cn"
    case global = "qoder_global"

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .cn: return "Qoder CN"
        case .global: return "Qoder International"
        }
    }

    var defaultProfileName: String {
        switch self {
        case .cn: return "Qoder CN 账号"
        case .global: return "Qoder International 账号"
        }
    }
}

/// 「AI 账号」页的三段分组。WorkBuddy 走 `accounts`，两个 Qoder 走 Profile。
enum ConsoleAccountSection: String, CaseIterable, Identifiable {
    case workbuddy
    case qoderCN
    case qoderGlobal

    var id: String { rawValue }

    var title: String {
        switch self {
        case .workbuddy: return "WorkBuddy"
        case .qoderCN: return "Qoder CN"
        case .qoderGlobal: return "Qoder International"
        }
    }

    var subtitle: String {
        switch self {
        case .workbuddy: return "扫码或从 Magpie 导入的 WorkBuddy 账号"
        case .qoderCN: return "国内版 Qoder，一个账号一份认证快照"
        case .qoderGlobal: return "国际版 Qoder，快照与 CN 分开存放"
        }
    }

    var edition: QoderEdition? {
        switch self {
        case .workbuddy: return nil
        case .qoderCN: return .cn
        case .qoderGlobal: return .global
        }
    }
}

/// 未完成的切换日志（只读接口报出来的，恢复需要显式确认）。
struct QoderPendingSwitch: Identifiable, Equatable {
    let id: String
    let profileId: String
    let edition: String
    let phase: String

    init?(json: [String: Any]) {
        guard let id = json["journalId"] as? String else { return nil }
        self.id = id
        self.profileId = json["profileId"] as? String ?? ""
        self.edition = json["edition"] as? String ?? ""
        self.phase = json["phase"] as? String ?? ""
    }

    var editionName: String {
        QoderEdition(rawValue: edition)?.displayName ?? edition
    }
}
