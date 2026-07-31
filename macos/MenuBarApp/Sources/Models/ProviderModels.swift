import CoreGraphics
import Foundation

// MARK: - Dynamic Provider Manifest Models (schema v1)

/// `GET /api/providers` response. Unknown fields are ignored; missing
/// optional fields degrade to safe defaults so one bad provider never
/// breaks the whole dashboard.
struct ProvidersResponse: Decodable, Equatable {
    let schemaVersion: Int
    let updatedAt: String?
    let providers: [ProviderSummary]

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        schemaVersion = (try? container.decodeIfPresent(Int.self, forKey: .schemaVersion)) ?? 1
        updatedAt = try? container.decodeIfPresent(String.self, forKey: .updatedAt)
        providers = (try? container.decodeIfPresent([ProviderSummary].self, forKey: .providers)) ?? []
    }

    private enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case updatedAt = "updated_at"
        case providers
    }
}

struct ProviderSummary: Decodable, Identifiable, Equatable {
    let id: String
    let displayName: String
    let category: String
    let icon: String?
    let state: String
    let available: Bool
    let stale: Bool
    let updatedAt: String?
    let sortOrder: Int
    let capabilities: [String]
    let metrics: [ProviderMetric]
    let actions: [ProviderAction]

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = (try? container.decodeIfPresent(String.self, forKey: .id)) ?? ""
        displayName = (try? container.decodeIfPresent(String.self, forKey: .displayName)) ?? id
        category = (try? container.decodeIfPresent(String.self, forKey: .category)) ?? "generic"
        icon = try? container.decodeIfPresent(String.self, forKey: .icon)
        state = (try? container.decodeIfPresent(String.self, forKey: .state)) ?? "unknown"
        available = (try? container.decodeIfPresent(Bool.self, forKey: .available)) ?? false
        stale = (try? container.decodeIfPresent(Bool.self, forKey: .stale)) ?? false
        updatedAt = try? container.decodeIfPresent(String.self, forKey: .updatedAt)
        sortOrder = (try? container.decodeIfPresent(Int.self, forKey: .sortOrder)) ?? 100
        capabilities = (try? container.decodeIfPresent([String].self, forKey: .capabilities)) ?? []
        metrics = (try? container.decodeIfPresent([ProviderMetric].self, forKey: .metrics)) ?? []
        actions = (try? container.decodeIfPresent([ProviderAction].self, forKey: .actions)) ?? []
    }

    /// First primary metric drives the large card number.
    var primaryMetrics: [ProviderMetric] {
        metrics.filter(\.primary)
    }

    /// Secondary metrics shown as small rows (the card caps this at three).
    var secondaryMetrics: [ProviderMetric] {
        metrics.filter { !$0.primary }
    }

    private enum CodingKeys: String, CodingKey {
        case id
        case displayName = "display_name"
        case category
        case icon
        case state
        case available
        case stale
        case updatedAt = "updated_at"
        case sortOrder = "sort_order"
        case capabilities
        case metrics
        case actions
    }
}

struct ProviderMetric: Decodable, Identifiable, Equatable {
    let key: String
    let label: String
    let value: MetricValue?
    let valueType: String
    let format: String
    let unit: String?
    let primary: Bool

    var id: String { key }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        key = (try? container.decodeIfPresent(String.self, forKey: .key)) ?? ""
        label = (try? container.decodeIfPresent(String.self, forKey: .label)) ?? key
        value = try? container.decodeIfPresent(MetricValue.self, forKey: .value)
        valueType = (try? container.decodeIfPresent(String.self, forKey: .valueType)) ?? "text"
        format = (try? container.decodeIfPresent(String.self, forKey: .format)) ?? "plain"
        unit = try? container.decodeIfPresent(String.self, forKey: .unit)
        primary = (try? container.decodeIfPresent(Bool.self, forKey: .primary)) ?? false
    }

    /// Unknown metric types safely degrade to plain text display.
    var safeValueType: String {
        ["number", "percentage", "currency", "text", "status", "duration"].contains(valueType)
            ? valueType
            : "text"
    }

    var safeFormat: String {
        ["integer", "decimal", "percent", "currency", "plain", "compact"].contains(format)
            ? format
            : "plain"
    }

    private enum CodingKeys: String, CodingKey {
        case key
        case label
        case value
        case valueType = "value_type"
        case format
        case unit
        case primary
    }
}

/// Lossy value container: Int, Double, String, Bool and null all decode
/// without failing the surrounding metric.
enum MetricValue: Decodable, Encodable, Equatable {
    case number(Double)
    case text(String)
    case boolean(Bool)
    case null

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
            return
        }
        if let value = try? container.decode(Double.self) {
            self = .number(value)
            return
        }
        if let value = try? container.decode(Bool.self) {
            self = .boolean(value)
            return
        }
        if let value = try? container.decode(String.self) {
            self = .text(value)
            return
        }
        self = .null
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .number(let value): try container.encode(value)
        case .text(let value): try container.encode(value)
        case .boolean(let value): try container.encode(value)
        case .null: try container.encodeNil()
        }
    }

    var numericValue: Double? {
        switch self {
        case .number(let value): return value
        case .text(let value): return Double(value)
        default: return nil
        }
    }
}

struct ProviderAction: Decodable, Identifiable, Equatable {
    let actionID: String
    let label: String
    let kind: String
    let localOnly: Bool

    var id: String { actionID }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        actionID = (try? container.decodeIfPresent(String.self, forKey: .actionID)) ?? ""
        label = (try? container.decodeIfPresent(String.self, forKey: .label)) ?? actionID
        kind = (try? container.decodeIfPresent(String.self, forKey: .kind)) ?? ""
        localOnly = (try? container.decodeIfPresent(Bool.self, forKey: .localOnly)) ?? true
    }

    private enum CodingKeys: String, CodingKey {
        case actionID = "id"
        case label
        case kind
        case localOnly = "local_only"
    }
}

// MARK: - Metric formatting

struct MetricDisplay: Equatable {
    let number: String?
    let unit: String
    /// True when the metric has no usable value (null / unknown type).
    let placeholder: Bool

    static let empty = MetricDisplay(number: nil, unit: "", placeholder: true)
}

enum MetricFormatter {
    /// Format a manifest metric for display without scientific notation,
    /// without truncation, and without ever shrinking to unreadable text.
    static func format(
        value: MetricValue?,
        valueType: String,
        format: String,
        unit: String?
    ) -> MetricDisplay {
        guard let value else { return .empty }
        let unitText = unit ?? ""
        switch value {
        case .null:
            return .empty
        case .boolean(let flag):
            return MetricDisplay(number: flag ? "Yes" : "No", unit: "", placeholder: false)
        case .text(let text):
            return MetricDisplay(number: text, unit: "", placeholder: false)
        case .number(let number):
            switch valueType {
            case "percentage", "percent":
                return MetricDisplay(number: integerString(number), unit: unitText.isEmpty ? "%" : unitText, placeholder: false)
            case "currency":
                return MetricDisplay(
                    number: decimalString(number, maxFractionDigits: 2, minFractionDigits: 2, grouping: true),
                    unit: unitText,
                    placeholder: false
                )
            case "duration":
                return MetricDisplay(number: durationString(number), unit: "", placeholder: false)
            case "text", "status":
                return MetricDisplay(number: trimmedString(number), unit: "", placeholder: false)
            default:
                return formatNumber(number, format: format, unit: unitText)
            }
        }
    }

    private static func formatNumber(_ number: Double, format: String, unit: String) -> MetricDisplay {
        switch format {
        case "integer":
            return MetricDisplay(number: integerString(number), unit: unit, placeholder: false)
        case "percent":
            return MetricDisplay(number: integerString(number), unit: unit.isEmpty ? "%" : unit, placeholder: false)
        case "currency":
            return MetricDisplay(
                number: decimalString(number, maxFractionDigits: 2, minFractionDigits: 2, grouping: true),
                unit: unit,
                placeholder: false
            )
        case "compact":
            return MetricDisplay(number: compactString(number), unit: unit, placeholder: false)
        case "plain":
            return MetricDisplay(number: decimalString(number, maxFractionDigits: 2, minFractionDigits: 0, grouping: false), unit: unit, placeholder: false)
        default: // decimal
            let digits = significantDigits(number)
            // Long numbers keep their grouping and drop decimals instead of
            // shrinking the font or switching to scientific notation.
            let maxFraction = digits > 9 ? 0 : 2
            return MetricDisplay(
                number: decimalString(number, maxFractionDigits: maxFraction, minFractionDigits: 0, grouping: true),
                unit: unit,
                placeholder: false
            )
        }
    }

    static func integerString(_ number: Double) -> String {
        decimalString(number.rounded(), maxFractionDigits: 0, minFractionDigits: 0, grouping: true)
    }

    static func decimalString(
        _ number: Double,
        maxFractionDigits: Int,
        minFractionDigits: Int,
        grouping: Bool
    ) -> String {
        let formatter = NumberFormatter()
        formatter.numberStyle = .decimal
        formatter.usesGroupingSeparator = grouping
        formatter.maximumFractionDigits = maxFractionDigits
        formatter.minimumFractionDigits = minFractionDigits
        formatter.locale = Locale(identifier: "en_US_POSIX")
        return formatter.string(from: NSNumber(value: number)) ?? String(number)
    }

    static func compactString(_ number: Double) -> String {
        let absolute = abs(number)
        let value: Double
        let suffix: String
        if absolute >= 1_000_000_000 {
            value = number / 1_000_000_000
            suffix = "B"
        } else if absolute >= 1_000_000 {
            value = number / 1_000_000
            suffix = "M"
        } else if absolute >= 1_000 {
            value = number / 1_000
            suffix = "K"
        } else {
            return decimalString(number, maxFractionDigits: 2, minFractionDigits: 0, grouping: true)
        }
        let trimmed = decimalString(value, maxFractionDigits: 2, minFractionDigits: 0, grouping: true)
        return trimmed + suffix
    }

    static func durationString(_ seconds: Double) -> String {
        let total = max(0, Int(seconds.rounded()))
        if total >= 3600 {
            return String(format: "%dh %02dm", total / 3600, (total % 3600) / 60)
        }
        if total >= 60 {
            return String(format: "%dm %02ds", total / 60, total % 60)
        }
        return "\(total)s"
    }

    private static func trimmedString(_ number: Double) -> String {
        if number.rounded() == number {
            return integerString(number)
        }
        return decimalString(number, maxFractionDigits: 2, minFractionDigits: 0, grouping: true)
    }

    /// Significant digits before the decimal separator (grouping ignored).
    static func significantDigits(_ number: Double) -> Int {
        let absolute = abs(number)
        if absolute < 1 { return 0 }
        return Int(floor(log10(absolute))) + 1
    }
}

// MARK: - Typography tokens

/// Single source of truth for quota typography. Cards never scatter magic
/// numbers; adaptivity is computed here from the number length and mode.
enum DashboardTypography {
    static let primaryMetricLarge: CGFloat = 36
    static let primaryMetricCompact: CGFloat = 30
    static let secondaryMetric: CGFloat = 16
    static let metricLabel: CGFloat = 12
    static let unit: CGFloat = 13
    static let status: CGFloat = 11
    static let timestamp: CGFloat = 10

    /// Adaptive primary font size. Keeps the number readable instead of
    /// shrinking endlessly: long numbers drop decimals before font size.
    static func primaryFontSize(number: String, compact: Bool = false) -> CGFloat {
        let base = compact ? primaryMetricCompact : primaryMetricLarge
        switch significantCharacters(number) {
        case ..<5: return base
        case 5...6: return base - 2
        case 7...9: return base - 4
        default: return compact ? 24 : 30
        }
    }

    private static func significantCharacters(_ number: String) -> Int {
        number.filter { $0.isNumber }.count
    }
}

// MARK: - Provider preferences (pure logic, testable without AppKit)

enum ProviderPreferences {
    static let defaultOrder = ["codex", "workbuddy", "deepseek", "system"]

    /// Migrate the legacy per-provider visibility toggles into the dynamic
    /// hidden-provider collection exactly once.
    static func hiddenAfterMigration(
        showCodexStatus: Bool,
        showWorkBuddy: Bool,
        showDeepSeek: Bool
    ) -> Set<String> {
        var hidden = Set<String>()
        if !showCodexStatus { hidden.insert("codex") }
        if !showWorkBuddy { hidden.insert("workbuddy") }
        if !showDeepSeek { hidden.insert("deepseek") }
        return hidden
    }

    /// Sort providers by the user's stored order, then by manifest sort
    /// order. Unknown providers are appended after the known prefix.
    static func ordered<T>(
        _ providers: [T],
        order: [String],
        id: (T) -> String,
        manifestSortOrder: (T) -> Int
    ) -> [T] {
        providers.sorted { lhs, rhs in
            let lhsIndex = order.firstIndex(of: id(lhs)) ?? Int.max
            let rhsIndex = order.firstIndex(of: id(rhs)) ?? Int.max
            if lhsIndex != rhsIndex {
                return lhsIndex < rhsIndex
            }
            return manifestSortOrder(lhs) < manifestSortOrder(rhs)
        }
    }
}
