import Foundation
import CryptoKit

/// AICC's cross-platform quota contract (schema 3).
///
/// Magpie owns every provider sign-in and every quota read.  This file only
/// normalizes what `GET /v1/magpie/quotas` returns, so a provider added to
/// Magpie tomorrow (built-in or community plugin) shows up without a new
/// collector here.  `collectors/magpie.py` and `DashboardData.java` implement
/// the same rules and must be changed together.
enum QuotaKind: String, Codable, Sendable {
    case subscription
    case plan
    case balance
}

struct QuotaBalancePart: Codable, Equatable, Sendable {
    let label: String?
    let text: String
    let percent: Double?
}

struct QuotaItem: Codable, Equatable, Sendable {
    let name: String
    let used: Double?
    let remaining: Double?
    /// What is left in the vendor's own unit ("4705" credits, "91%"), already
    /// normalized by the bridge and by `MagpieQuotaParser`.
    let remainingText: String?
    let display: String?
    let amount: Double?
    let limit: Double?
    let unit: String?
    let unlimited: Bool
    let reset: String?
    let resetsAt: Double?
    let pool: String?

    enum CodingKeys: String, CodingKey {
        case name, used, remaining, display, amount, limit, unit, unlimited, reset, pool
        case remainingText = "remaining_text"
        case resetsAt = "resets_at"
    }

    /// What is left: the vendor's count when it counts in one, else its own
    /// text, else the share.  Never "used / total".
    var primaryText: String {
        if unlimited { return "∞" }
        if let remainingText, !remainingText.isEmpty { return remainingText }
        if let display, !display.isEmpty { return display }
        if let remaining { return String(format: "%.0f%%", remaining) }
        return "—"
    }

    var isPercentage: Bool { !unlimited && remaining != nil && remainingText?.hasSuffix("%") != false }

    /// "共 4723" for a counted window, nil otherwise.
    var totalText: String? {
        guard !unlimited, let limit, limit > 0, unit != nil else { return nil }
        return "共 " + String(format: "%.0f", limit)
    }

    var secondaryText: String? {
        guard let reset else { return nil }
        return reset
    }
}

struct QuotaCard: Codable, Equatable, Sendable, Identifiable {
    let id: String
    let provider: String
    let providerName: String
    let accountLabel: String
    let kind: QuotaKind
    let plan: String?
    let items: [QuotaItem]
    let balance: String?
    let balanceParts: [QuotaBalancePart]
    let resetsCount: Int?
    let renew: String?
    let lastServedAt: Double?
    let last: Bool
    let failed: Bool
    let deferred: Bool
    var stale: Bool
    let readAt: Double?

    enum CodingKeys: String, CodingKey {
        case id, provider, kind, plan, items, balance, stale, last, failed, deferred
        case providerName = "provider_name"
        case accountLabel = "account_label"
        case balanceParts = "balance_parts"
        case resetsCount = "resets_count"
        case renew
        case lastServedAt = "last_served_at"
        case readAt = "read_at"
    }

    var title: String { "\(providerName) · \(accountLabel)" }

    /// Stable per-window key for the user's display selection.  Window names are
    /// the vendor's and may repeat, so the pool is part of the identity.
    func key(for item: QuotaItem) -> String {
        "\(id)|\(item.name)|\(item.pool ?? "")"
    }
    var primaryItem: QuotaItem? { items.first }
    var secondaryItem: QuotaItem? { items.count > 1 ? items[1] : nil }
    var extraWindows: Int { max(0, items.count - 2) }
    var hasQuota: Bool { !items.isEmpty || (balance?.isEmpty == false) || !balanceParts.isEmpty }
    var isBalance: Bool { kind == .balance }

    var statusText: String? {
        if failed { return "读取失败" }
        if deferred { return "本轮跳过" }
        if stale { return "缓存" }
        return nil
    }

    /// A balance card drawn through the same slot shape as a window card.
    var balanceItem: QuotaItem {
        QuotaItem(name: "余额", used: nil, remaining: nil, remainingText: balanceText,
                  display: nil, amount: nil, limit: nil, unit: nil, unlimited: false,
                  reset: nil, resetsAt: nil, pool: nil)
    }

    var balanceText: String {
        if let balance, !balance.isEmpty { return balance }
        if !balanceParts.isEmpty { return balanceParts.map { $0.text }.joined(separator: " · ") }
        return "—"
    }

    func aged(at now: Date) -> QuotaCard {
        guard let readAt, now.timeIntervalSince1970 - readAt >= QuotaSnapshot.liveLifetime else { return self }
        var copy = self
        copy.stale = true
        return copy
    }
}

/// One drawn card: an account together with the window(s) the user picked.
struct QuotaSlot: Equatable, Sendable {
    let card: QuotaCard
    let primary: QuotaItem
    let secondary: QuotaItem?
    let extra: Int

    var id: String { card.id }
}

struct QuotaSnapshot: Codable, Equatable, Sendable {
    static let liveLifetime: TimeInterval = 5 * 60
    static let retention: TimeInterval = 24 * 60 * 60

    var schemaVersion: Int
    var source: String
    var available: Bool
    var state: String
    var stale: Bool
    var providers: [String]
    var accounts: [QuotaCard]
    var updatedAt: String?
    var updatedEpoch: Double?
    var refreshSeconds: Int
    var fetchedAt: Double

    enum CodingKeys: String, CodingKey {
        case source, available, state, stale, providers, accounts
        case schemaVersion = "schema_version"
        case updatedAt = "updated_at"
        case updatedEpoch = "updated_epoch"
        case refreshSeconds = "refresh_seconds"
        case fetchedAt = "fetched_at"
    }

    init(schemaVersion: Int = 3, source: String = "Magpie", available: Bool = false, state: String = "Empty",
         stale: Bool = true, providers: [String] = [], accounts: [QuotaCard] = [], updatedAt: String? = nil,
         updatedEpoch: Double? = nil, refreshSeconds: Int = 300, fetchedAt: Double = Date.now.timeIntervalSince1970) {
        self.schemaVersion = schemaVersion
        self.source = source
        self.available = available
        self.state = state
        self.stale = stale
        self.providers = providers
        self.accounts = accounts
        self.updatedAt = updatedAt
        self.updatedEpoch = updatedEpoch
        self.refreshSeconds = refreshSeconds
        self.fetchedAt = fetchedAt
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        schemaVersion = try container.decodeIfPresent(Int.self, forKey: .schemaVersion) ?? 3
        source = try container.decodeIfPresent(String.self, forKey: .source) ?? "Magpie"
        available = try container.decodeIfPresent(Bool.self, forKey: .available) ?? false
        state = try container.decodeIfPresent(String.self, forKey: .state) ?? "Empty"
        stale = try container.decodeIfPresent(Bool.self, forKey: .stale) ?? true
        providers = try container.decodeIfPresent([String].self, forKey: .providers) ?? []
        accounts = try container.decodeIfPresent([QuotaCard].self, forKey: .accounts) ?? []
        updatedAt = try container.decodeIfPresent(String.self, forKey: .updatedAt)
        updatedEpoch = try container.decodeIfPresent(Double.self, forKey: .updatedEpoch)
        refreshSeconds = try container.decodeIfPresent(Int.self, forKey: .refreshSeconds) ?? 300
        fetchedAt = try container.decodeIfPresent(Double.self, forKey: .fetchedAt) ?? Date.now.timeIntervalSince1970
    }

    var isEmpty: Bool { accounts.isEmpty }

    func age(at now: Date = .now) -> TimeInterval {
        max(0, now.timeIntervalSince1970 - fetchedAt)
    }

    /// Freshness for the moment the timeline draws, never a claim of live data.
    func evaluated(at now: Date = .now, offline: Bool = false) -> QuotaSnapshot {
        if age(at: now) >= Self.retention { return .placeholder() }
        var copy = self
        copy.accounts = accounts.map { $0.aged(at: now) }
        if offline || age(at: now) >= Self.liveLifetime {
            copy.stale = true
            copy.state = accounts.isEmpty ? "Empty" : "Cached"
            copy.accounts = copy.accounts.map { card in
                var card = card
                card.stale = true
                return card
            }
        }
        return copy
    }

    /// What to draw, honouring the user's window selection, the optional
    /// provider filter and the family's capacity.
    ///
    /// A selection is a list of `QuotaCard.key(for:)` values in the order the
    /// user ticked them.  A card shows at most two of its selected windows, so
    /// the card keeps its shape while the choice is per window.  A key that no
    /// longer exists (a vendor renamed its window) is skipped rather than
    /// blanking the widget; no usable key at all falls back to automatic.
    func slots(selection: [String], provider: String? = nil, limit: Int) -> (shown: [QuotaSlot], hidden: Int) {
        var pool = accounts
        if let provider, !provider.isEmpty {
            pool = pool.filter { $0.provider == provider }
        }
        var cards: [QuotaCard] = []
        var chosen: [String: [QuotaItem]] = [:]
        if !selection.isEmpty {
            var byKey: [String: (QuotaCard, QuotaItem)] = [:]
            for card in pool {
                for item in card.items { byKey[card.key(for: item)] = (card, item) }
            }
            for key in selection {
                guard let (card, item) = byKey[key] else { continue }
                if chosen[card.id] == nil {
                    cards.append(card)
                    chosen[card.id] = []
                }
                guard let picked = chosen[card.id], picked.count < 2 else { continue }
                if picked.contains(where: { $0.name == item.name && $0.pool == item.pool }) { continue }
                chosen[card.id] = picked + [item]
            }
        }
        if cards.isEmpty {
            cards = pool
            for card in pool { chosen[card.id] = Array(card.items.prefix(2)) }
        }
        // Last served first, then the order the user chose.
        let ranked = cards.enumerated().sorted { left, right in
            if left.element.last != right.element.last { return left.element.last }
            return left.offset < right.offset
        }.map { $0.element }

        let slots: [QuotaSlot] = ranked.compactMap { card in
            var items = chosen[card.id] ?? []
            // A balance card has no window: it is still a card, with the amount
            // standing in for the leading value rather than being dropped.
            if items.isEmpty, card.isBalance {
                items = [card.balanceItem]
            }
            guard let first = items.first else { return nil }
            return QuotaSlot(card: card, primary: first,
                             secondary: items.count > 1 ? items[1] : nil,
                             extra: max(0, card.items.count - items.count))
        }
        return (Array(slots.prefix(limit)), max(0, slots.count - limit))
    }

    static func placeholder() -> QuotaSnapshot {
        QuotaSnapshot(available: false, state: "Unavailable", stale: true, providers: [], accounts: [])
    }
}

enum WidgetCardDisplayMode: String, CaseIterable, Identifiable, Codable, Sendable {
    case automatic
    case one
    case two
    case four
    case all

    var id: String { rawValue }

    func displayName(localize: (String) -> String) -> String {
        switch self {
        case .automatic: return localize("Automatic")
        case .one: return localize("1 Card")
        case .two: return localize("2 Cards")
        case .four: return localize("4 Cards")
        case .all: return localize("All Cards")
        }
    }

    /// Pre-3.0 builds stored the same choice as `automatic|one|two|all`.
    static func migrated(_ legacy: String) -> WidgetCardDisplayMode {
        switch legacy {
        case "one": return .one
        case "two": return .two
        case "all": return .all
        case "four": return .four
        default: return .automatic
        }
    }

    func capacity(for familyLimit: Int) -> Int {
        switch self {
        case .automatic, .four: return familyLimit
        case .one: return 1
        case .two: return min(2, familyLimit)
        case .all: return Int.max
        }
    }
}

/// Parses Magpie's raw quota report.  The bridge (`schema_version` 3) carries
/// the same shape, so a cached bridge payload decodes with `JSONDecoder`.
enum MagpieQuotaParser {
    static func snapshot(data: Data, now: Date = .now, fetchedAt: Date = .now,
                         refreshSeconds: Int = 300) throws -> QuotaSnapshot {
        let cards = try parse(data: data, now: now)
        return QuotaSnapshot(available: !cards.isEmpty, state: state(of: cards), stale: !cards.isEmpty && cards.allSatisfy(\.stale),
                             providers: providers(of: cards), accounts: cards,
                             updatedAt: cards.compactMap(\.readAt).max().map(clock), updatedEpoch: cards.compactMap(\.readAt).max(),
                             refreshSeconds: refreshSeconds, fetchedAt: fetchedAt.timeIntervalSince1970)
    }

    static func parse(data: Data, now: Date = .now) throws -> [QuotaCard] {
        guard let document = try JSONSerialization.jsonObject(with: data) as? [String: Any],
              document["object"] as? String == "list",
              let rows = document["data"] as? [[String: Any]] else {
            throw CocoaError(.coderReadCorrupt)
        }
        let stamp = now.timeIntervalSince1970
        var cards: [QuotaCard] = []
        for row in rows {
            guard let card = card(from: row, now: stamp) else { continue }
            if let index = cards.firstIndex(where: { $0.id == card.id }) {
                if (card.readAt ?? 0) > (cards[index].readAt ?? 0) { cards[index] = card }
                continue
            }
            cards.append(card)
        }
        return cards
    }

    static func card(from row: [String: Any], now: Double) -> QuotaCard? {
        guard let provider = (row["provider"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
              !provider.isEmpty, let kind = QuotaKind(rawValue: row["kind"] as? String ?? "") else { return nil }
        let user = row["user"] as? String ?? ""
        let origin = row["from"] as? String ?? ""
        let identity = sha256("\(origin)\u{0}\(provider)\u{0}\(kind.rawValue)\u{0}\(user.lowercased())")
        var items: [QuotaItem] = []
        if let windows = row["windows"] as? [[String: Any]] {
            items = windows.compactMap(item(from:))
        }
        let error = row["error"]
        let failed = (error as? String).map { !$0.isEmpty } ?? (error != nil && !(error is NSNull))
        let deferred = row["readDeferred"] as? Bool ?? false
        let asOf = date(row["asOf"])
        let readAt = date(row["readAt"]) ?? asOf
        var stale = failed || deferred || asOf != nil || readAt == nil
        if let readAt, now - readAt >= QuotaSnapshot.liveLifetime || readAt > now + 60 { stale = true }
        if items.contains(where: { ($0.resetsAt ?? .greatestFiniteMagnitude) <= now }) { stale = true }
        let resets = row["resets"] as? [String: Any]
        let plan = (row["plan"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
        let renew = row["renew"] as? String
        return QuotaCard(id: "magpie-" + identity,
                         provider: provider,
                         providerName: providerName(provider),
                         accountLabel: displayLabel(row),
                         kind: kind,
                         plan: (plan?.isEmpty == false) ? String(plan!.prefix(32)) : nil,
                         items: items,
                         balance: (row["balance"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
                         balanceParts: parts(row["balanceSplit"]),
                         resetsCount: resets?["count"] as? Int,
                         renew: (renew == "auto" || renew == "off") ? renew : nil,
                         lastServedAt: date(row["lastServedAt"]),
                         last: row["last"] as? Bool ?? false,
                         failed: failed,
                         deferred: deferred,
                         stale: stale,
                         readAt: readAt)
    }

    static func item(from window: [String: Any]) -> QuotaItem? {
        guard let raw = window["name"] as? String else { return nil }
        let name = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return nil }
        let unlimited = window["unlimited"] as? Bool == true
        let used = percent(window["used"])
        var remaining = unlimited ? nil : percent(window["remaining"])
        if remaining == nil, !unlimited, let used { remaining = percent(100 - used) }
        let display = (window["display"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
        let resetsAt = date(window["resetsAt"])
        let unitText = (window["unit"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
        let limitValue = number(window["limit"])
        var remainingText: String?
        if !unlimited {
            if let limitValue, let unitText, !unitText.isEmpty {
                // Magpie omits `amount` when nothing is used.
                var usedCount = number(window["amount"])
                if usedCount == nil, remaining == 100 || used == 0 { usedCount = 0 }
                if let usedCount { remainingText = count(max(0, limitValue - usedCount)) }
            }
            if remainingText == nil {
                if let display, !display.isEmpty {
                    remainingText = String(display.prefix(32))
                } else if let remaining {
                    remainingText = String(Int(remaining.rounded())) + "%"
                }
            }
        }
        return QuotaItem(name: String(name.prefix(40)),
                         used: unlimited ? nil : used,
                         remaining: remaining,
                         remainingText: remainingText,
                         display: (display?.isEmpty == false) ? String(display!.prefix(32)) : nil,
                         amount: number(window["amount"]),
                         limit: number(window["limit"]),
                         unit: unitText,
                         unlimited: unlimited,
                         reset: resetsAt.map(clock),
                         resetsAt: resetsAt,
                         pool: (window["pool"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines))
    }

    static func parts(_ value: Any?) -> [QuotaBalancePart] {
        guard let rows = value as? [[String: Any]] else { return [] }
        return rows.compactMap { row in
            guard let text = (row["text"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
                  !text.isEmpty else { return nil }
            let label = (row["label"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
            return QuotaBalancePart(label: (label?.isEmpty == false) ? label : nil, text: String(text.prefix(32)),
                                    percent: percent(row["percent"]))
        }
    }

    /// Same rule as `_count` in collectors/magpie.py: whole numbers, one
    /// decimal when the count is small.
    static func count(_ value: Double) -> String {
        if value >= 100 { return String(Int(value.rounded())) }
        var text = String(format: "%.1f", value)
        while text.hasSuffix("0") { text.removeLast() }
        if text.hasSuffix(".") { text.removeLast() }
        return text
    }

    static func number(_ value: Any?) -> Double? {
        guard let raw = value, !(raw is NSNull) else { return nil }
        if raw is Bool { return nil }
        guard let number = raw as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID() else { return nil }
        let double = number.doubleValue
        return double.isFinite ? double : nil
    }

    static func percent(_ value: Any?) -> Double? {
        guard let number = number(value), (0...100).contains(number) else { return nil }
        return (number * 100).rounded() / 100
    }

    static func date(_ value: Any?) -> Double? {
        guard let text = value as? String, !text.isEmpty else { return nil }
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let parsed = formatter.date(from: text) { return parsed.timeIntervalSince1970 }
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: text)?.timeIntervalSince1970
    }

    static func clock(_ epoch: Double) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd HH:mm"
        return formatter.string(from: Date(timeIntervalSince1970: epoch))
    }

    static func sha256(_ text: String) -> String {
        SHA256.hash(data: Data(text.utf8)).prefix(6).map { String(format: "%02x", $0) }.joined()
    }

    /// First character and nothing else: an email or an account's own name is
    /// never written into a snapshot, a cache or a screenshot.
    static func mask(_ text: String?) -> String? {
        guard let value = text?.trimmingCharacters(in: .whitespacesAndNewlines), !value.isEmpty else { return nil }
        let local = value.contains("@") ? String(value.split(separator: "@", maxSplits: 1)[0]) : value
        let trimmed = local.trimmingCharacters(in: CharacterSet(charactersIn: " <>()[]"))
        guard let first = trimmed.first else { return "***" }
        return String(first) + "***"
    }

    static func displayLabel(_ row: [String: Any]) -> String {
        for key in ["alias", "seat"] {
            if let value = (row[key] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
               !value.isEmpty, value.count <= 24, !value.contains("@") {
                return value
            }
        }
        return mask(row["user"] as? String) ?? "***"
    }

    static func providerName(_ provider: String) -> String {
        if let known = providerNames[provider] { return known }
        let words = provider.replacingOccurrences(of: "_", with: "-").split(separator: "-")
        let named = words.map { $0.prefix(1).uppercased() + $0.dropFirst() }.joined(separator: " ")
        return named.isEmpty ? "Unknown" : named
    }

    static let providerNames: [String: String] = [
        "codex": "Codex", "claude": "Claude", "anthropic": "Claude", "openai": "OpenAI",
        "gemini": "Gemini", "antigravity": "Antigravity", "workbuddy": "WorkBuddy",
        "workbuddy-ai": "WorkBuddy AI", "zcode": "ZCode", "kiro": "Kiro",
        "minimax-code": "MiniMax Code", "minimax-code-global": "MiniMax Code",
        "qoder": "Qoder", "qoder-cn": "Qoder CN", "devin": "Devin", "zed": "Zed",
        "factory": "Factory", "grok": "Grok", "cursor": "Cursor", "cline": "Cline",
        "trae-cn": "Trae", "trae-global": "Trae", "commandcode-plan": "Command Code",
        "mimo-app": "MiMo", "kimi-work": "Kimi Work", "opencode-zen-free": "OpenCode Zen",
        "opendesign": "Open Design", "raccoon": "Raccoon", "autoclaw": "AutoClaw",
        "autoclaw-cn": "AutoClaw CN",
    ]

    static func state(of cards: [QuotaCard]) -> String {
        if cards.isEmpty { return "Empty" }
        return cards.allSatisfy(\.stale) ? "Cached" : "Connected"
    }

    static func providers(of cards: [QuotaCard]) -> [String] {
        var ordered: [String] = []
        for card in cards where !ordered.contains(card.provider) { ordered.append(card.provider) }
        return ordered
    }
}
