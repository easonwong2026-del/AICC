import Foundation

/// 额度行的数据可靠性状态。模型层只说状态，颜色由视图映射到 ConsoleTheme。
enum QuotaStateTone: Equatable {
    case normal
    case neutral
    case warning
    case danger
}

enum QuotaDisplayState: Equatable {
    /// 接口读取成功，且已经拿到可用的额度数值。只有这一种状态配绿色点。
    case live
    /// 接口读取成功，但没有返回额度数值。它不等于失败，也不等于 0。
    case unknown
    case cached
    case stale
    case failed
    case deferred
    case unverified
    case unauthorized

    /// 正常状态不显示文字；异常才出现在状态列。
    var label: String? {
        switch self {
        case .live: return nil
        case .unknown: return "未取得额度"
        case .cached: return "缓存"
        case .stale: return "旧快照"
        case .failed: return "读取失败"
        case .deferred: return "本轮跳过"
        case .unverified: return "暂不支持"
        case .unauthorized: return "需要登录"
        }
    }

    var tone: QuotaStateTone {
        switch self {
        case .live: return .normal
        case .unknown: return .neutral
        case .cached, .stale: return .warning
        case .failed, .unauthorized: return .danger
        case .deferred, .unverified: return .neutral
        }
    }

    var help: String? {
        switch self {
        case .live: return "实时数据"
        case .unknown: return "接口读取成功，但本次没有返回可用的额度数值"
        case .cached: return "显示上一次读取的缓存"
        case .stale: return "本次没有取到新数据，显示旧快照"
        case .failed: return "这次读取失败，没有把未知显示成 0"
        case .deferred: return "本轮读取被跳过，稍后刷新"
        case .unverified: return "该平台没有已证实的额度接口，未发送请求"
        case .unauthorized: return "授权不可用，需要重新登录"
        }
    }

    static func worker(_ row: WorkerQuotaRow) -> QuotaDisplayState {
        switch row.status {
        case "ok":
            if row.stale { return .stale }
            if row.cached { return .cached }
            // 接口成功 ≠ 数值已取得：没有 remaining 就不能标成已确认。
            if row.remaining == nil { return .unknown }
            return .live
        case "stale": return .stale
        case "unverified": return .unverified
        case "unauthorized": return .unauthorized
        default: return .failed
        }
    }

    static func magpie(card: QuotaCard, snapshotStale: Bool) -> QuotaDisplayState {
        if card.failed { return .failed }
        if card.deferred { return .deferred }
        if card.stale || snapshotStale { return .stale }
        // 空卡（没有窗口也没有余额）同样是「没拿到数值」，不能用绿点。
        if !card.hasQuota { return .unknown }
        return .live
    }
}

/// 账号额度的一行展示。只做格式化，不发请求、不合并不同单位。
struct AccountQuotaDisplay: Identifiable, Equatable {
    let id: String
    let accountName: String
    let providerName: String
    let plan: String?
    /// 未知必须是「未知」或「—」，绝不显示 0。
    let remainingValue: String
    let unit: String?
    let remainingShare: Double?
    let remainingHelp: String?
    let usagePrimary: String
    let usageSecondary: String?
    let usageHelp: String?
    let updatedText: String?
    let state: QuotaDisplayState
    /// 每日奖励是活动状态，不是余额；只出现在账号副行。
    let creditsBadge: String?
    let creditsHelp: String?
    let creditsNavigatesToCheckin: Bool
    let sourceHelp: String

    init(row: WorkerQuotaRow) {
        id = row.id
        accountName = row.nickname.isEmpty ? row.accountId : row.nickname
        providerName = row.providerName
        plan = row.plan
        if row.hasQuota {
            remainingValue = row.remainingText ?? "未知"
            unit = row.remainingText == nil ? nil : row.unit
            remainingShare = row.remainingShare
            remainingHelp = row.totalText
        } else {
            remainingValue = "—"
            unit = nil
            remainingShare = nil
            remainingHelp = nil
        }

        if let usage = row.usage, usage.requests > 0 {
            usagePrimary = "\(usage.requests) 次请求"
            var parts = ["\(WorkerQuotaRow.format(usage.credits)) credits"]
            if usage.truncated { parts.append("已截断") }
            parts.append("近 \(usage.windowDays) 天")
            usageSecondary = parts.joined(separator: " · ")
            usageHelp = "近 \(usage.windowDays) 天的请求数与消耗；切换 7/30 天只改用量窗口，不改余额。"
        } else if let usageError = row.usageError, !usageError.isEmpty {
            usagePrimary = "读取失败"
            usageSecondary = nil
            usageHelp = usageError
        } else {
            usagePrimary = "—"
            usageSecondary = nil
            usageHelp = "这个窗口没有用量记录"
        }

        updatedText = QuotaPresentation.localTimestamp(row.capturedAt ?? row.syncAt)
        state = QuotaDisplayState.worker(row)
        if let credits = row.credits {
            creditsBadge = QuotaPresentation.creditsBadge(credits)
            var help = credits.summary + "。每日奖励不是余额，不与剩余额度相加。"
            if let end = credits.windowEnd,
               let endText = QuotaPresentation.localTime(epochSeconds: end) {
                help += "活动窗口截止 \(endText)（服务端时间）。"
            }
            creditsHelp = help
            creditsNavigatesToCheckin = true
        } else {
            creditsBadge = nil
            creditsHelp = nil
            creditsNavigatesToCheckin = false
        }
        sourceHelp = "数据来源：\(row.sourceName)"
    }
}

/// Magpie 额度的一行展示：一个账号的一个窗口（余额卡是一行）。
struct MagpieQuotaDisplayRow: Identifiable, Equatable {
    let id: String
    let accountName: String
    let plan: String?
    let windowName: String
    let remaining: String
    let unit: String?
    let remainingShare: Double?
    let usedText: String?
    let reset: String?
    let state: QuotaDisplayState
    let detail: String?
    let isFirstRow: Bool

    static func rows(for card: QuotaCard, snapshotStale: Bool) -> [MagpieQuotaDisplayRow] {
        let state = QuotaDisplayState.magpie(card: card, snapshotStale: snapshotStale)
        if card.isBalance {
            let parts = card.balanceParts.map { part in
                [part.label, part.text].compactMap { $0 }.joined(separator: " ")
            }.joined(separator: " · ")
            return [MagpieQuotaDisplayRow(
                id: "\(card.id)|balance",
                accountName: card.accountLabel,
                plan: card.plan,
                windowName: "余额",
                remaining: card.balanceText,
                unit: nil,
                remainingShare: nil,
                usedText: nil,
                reset: nil,
                state: state,
                detail: parts.isEmpty ? nil : parts,
                isFirstRow: true
            )]
        }
        guard !card.items.isEmpty else {
            return [MagpieQuotaDisplayRow(
                id: "\(card.id)|empty",
                accountName: card.accountLabel,
                plan: card.plan,
                windowName: "无额度窗口",
                remaining: "—",
                unit: nil,
                remainingShare: nil,
                usedText: nil,
                reset: nil,
                state: state,
                detail: nil,
                isFirstRow: true
            )]
        }
        return card.items.enumerated().map { index, item in
            let percentage = item.remainingText?.hasSuffix("%") == true
            return MagpieQuotaDisplayRow(
                id: "\(card.id)|\(item.name)|\(item.pool ?? String(index))",
                accountName: card.accountLabel,
                plan: card.plan,
                windowName: item.name,
                remaining: item.primaryText,
                unit: percentage ? nil : item.unit,
                remainingShare: item.unlimited ? nil : item.remaining,
                usedText: item.used.map { "\(Int($0.rounded()))%" },
                reset: QuotaPresentation.compactReset(item.reset),
                state: state,
                detail: [item.totalText, percentage ? nil : item.unit]
                    .compactMap { $0 }
                    .joined(separator: " ")
                    .nilIfEmpty,
                isFirstRow: index == 0
            )
        }
    }
}

enum QuotaPresentation {
    /// RFC3339 → 本机 `MM-dd HH:mm`。解析失败返回 `nil`，不猜时间。
    static func localTimestamp(_ raw: String?) -> String? {
        guard let raw, !raw.isEmpty else { return nil }
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        var date = parser.date(from: raw)
        if date == nil {
            parser.formatOptions = [.withInternetDateTime]
            date = parser.date(from: raw)
        }
        guard let date else { return nil }
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.dateFormat = "MM-dd HH:mm"
        return formatter.string(from: date)
    }

    /// Unix 秒 → 本机 `MM-dd HH:mm`（服务端活动窗口）。
    static func localTime(epochSeconds: Int) -> String? {
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.dateFormat = "MM-dd HH:mm"
        return formatter.string(from: Date(timeIntervalSince1970: TimeInterval(epochSeconds)))
    }

    /// Magpie 的 reset 可能是 `yyyy-MM-dd HH:mm`，表格里只显示 `MM-dd HH:mm`。
    static func compactReset(_ raw: String?) -> String? {
        guard let raw, !raw.isEmpty else { return nil }
        guard raw.count >= 16 else { return raw }
        let parser = DateFormatter()
        parser.locale = Locale(identifier: "en_US_POSIX")
        parser.dateFormat = "yyyy-MM-dd HH:mm"
        guard let date = parser.date(from: String(raw.prefix(16))) else { return raw }
        let formatter = DateFormatter()
        formatter.locale = .current
        formatter.dateFormat = "MM-dd HH:mm"
        return formatter.string(from: date)
    }

    /// 每日奖励的短标签。它永远不出现在剩余额度列。
    static func creditsBadge(_ credits: QoderCreditsInfo) -> String {
        switch credits.state {
        case "claimable":
            if let amount = credits.todayCredits {
                return "每日奖励 · 可领 \(WorkerQuotaRow.format(amount)) Credits"
            }
            return "每日奖励 · 可领"
        case "already": return "每日奖励 · 已领取"
        case "nothing": return "每日奖励 · 今日无"
        case "no_credential": return "每日奖励 · 需要登录"
        case "unsupported": return "每日奖励 · 暂不支持"
        case "unauthorized": return "每日奖励 · 需要登录"
        case "busy": return "每日奖励 · 领取中"
        default: return "每日奖励 · 查询失败"
        }
    }
}

private extension String {
    var nilIfEmpty: String? { isEmpty ? nil : self }
}
