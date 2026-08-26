import CoreGraphics

// MARK: - Typography tokens

/// Single source of truth for the fixed dashboard cards.
enum DashboardTypography {
    static let primaryMetricLarge: CGFloat = 36
    static let primaryMetricCompact: CGFloat = 30
    static let secondaryMetric: CGFloat = 16
    static let metricLabel: CGFloat = 12
    static let unit: CGFloat = 13
    static let status: CGFloat = 11
    static let timestamp: CGFloat = 10

    /// Keep quota values readable instead of shrinking them indefinitely.
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
