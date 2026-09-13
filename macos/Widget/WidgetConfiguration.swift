public enum AICCWidgetMetadata {
    public static let kind = "com.aieink.dashboard.menubar.widget"
}

import Foundation
import AppIntents
import WidgetKit

public enum WidgetMetricOption: String, CaseIterable, Codable, Sendable, AppEnum {
    case codex = "codex"
    case google = "google"
    case workbuddy = "workbuddy"
    case deepseek = "deepseek"

    public static var typeDisplayRepresentation: TypeDisplayRepresentation {
        TypeDisplayRepresentation(name: LocalizedStringResource("Metric", defaultValue: "Metric"))
    }

    public static var caseDisplayRepresentations: [WidgetMetricOption: DisplayRepresentation] {
        [
            .codex: DisplayRepresentation(title: "Codex"),
            .google: DisplayRepresentation(title: "Google"),
            .workbuddy: DisplayRepresentation(title: "WorkBuddy"),
            .deepseek: DisplayRepresentation(title: "DeepSeek")
        ]
    }

    public var displayName: String {
        switch self {
        case .codex: return "Codex"
        case .google: return "Google"
        case .workbuddy: return "WorkBuddy"
        case .deepseek: return "DeepSeek"
        }
    }

    public func localizedName(isChinese: Bool) -> String {
        switch self {
        case .codex: return "Codex"
        case .google: return "Google"
        case .workbuddy: return "WorkBuddy"
        case .deepseek: return "DeepSeek"
        }
    }

    public var isQuota: Bool {
        switch self {
        case .codex, .google: return true
        case .workbuddy, .deepseek: return false
        }
    }
}

public struct AICCWidgetConfigurationIntent: WidgetConfigurationIntent, Codable, Equatable, Sendable {
    public static var title: LocalizedStringResource = "AICC Widget Configuration"
    public static var description = IntentDescription("Customize displayed metrics in AICC widget")

    @Parameter(title: "Primary Metric", default: nil)
    public var primaryMetric: WidgetMetricOption?

    @Parameter(title: "Secondary Metric", default: nil)
    public var secondaryMetric: WidgetMetricOption?

    @Parameter(title: "Top Left", default: .codex)
    public var topLeft: WidgetMetricOption

    @Parameter(title: "Top Right", default: .google)
    public var topRight: WidgetMetricOption

    @Parameter(title: "Bottom Left", default: .workbuddy)
    public var bottomLeft: WidgetMetricOption

    @Parameter(title: "Bottom Right", default: .deepseek)
    public var bottomRight: WidgetMetricOption

    public init() {
        self.primaryMetric = nil
        self.secondaryMetric = nil
        self.topLeft = .codex
        self.topRight = .google
        self.bottomLeft = .workbuddy
        self.bottomRight = .deepseek
    }

    public init(primary: WidgetMetricOption, secondary: WidgetMetricOption) {
        self.primaryMetric = primary
        self.secondaryMetric = secondary
        self.topLeft = primary
        self.topRight = secondary
        self.bottomLeft = .workbuddy
        self.bottomRight = .deepseek
    }

    public init(topLeft: WidgetMetricOption = .codex,
                topRight: WidgetMetricOption = .google,
                bottomLeft: WidgetMetricOption = .workbuddy,
                bottomRight: WidgetMetricOption = .deepseek) {
        self.primaryMetric = nil
        self.secondaryMetric = nil
        self.topLeft = topLeft
        self.topRight = topRight
        self.bottomLeft = bottomLeft
        self.bottomRight = bottomRight
    }

    enum CodingKeys: String, CodingKey {
        case primaryMetric
        case secondaryMetric
        case topLeft
        case topRight
        case bottomLeft
        case bottomRight
    }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.primaryMetric = try container.decodeIfPresent(WidgetMetricOption.self, forKey: .primaryMetric)
        self.secondaryMetric = try container.decodeIfPresent(WidgetMetricOption.self, forKey: .secondaryMetric)
        self.topLeft = try container.decodeIfPresent(WidgetMetricOption.self, forKey: .topLeft) ?? .codex
        self.topRight = try container.decodeIfPresent(WidgetMetricOption.self, forKey: .topRight) ?? .google
        self.bottomLeft = try container.decodeIfPresent(WidgetMetricOption.self, forKey: .bottomLeft) ?? .workbuddy
        self.bottomRight = try container.decodeIfPresent(WidgetMetricOption.self, forKey: .bottomRight) ?? .deepseek
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encodeIfPresent(primaryMetric, forKey: .primaryMetric)
        try container.encodeIfPresent(secondaryMetric, forKey: .secondaryMetric)
        try container.encode(topLeft, forKey: .topLeft)
        try container.encode(topRight, forKey: .topRight)
        try container.encode(bottomLeft, forKey: .bottomLeft)
        try container.encode(bottomRight, forKey: .bottomRight)
    }

    public static func == (lhs: AICCWidgetConfigurationIntent, rhs: AICCWidgetConfigurationIntent) -> Bool {
        lhs.primaryMetric == rhs.primaryMetric &&
        lhs.secondaryMetric == rhs.secondaryMetric &&
        lhs.topLeft == rhs.topLeft &&
        lhs.topRight == rhs.topRight &&
        lhs.bottomLeft == rhs.bottomLeft &&
        lhs.bottomRight == rhs.bottomRight
    }

    /// Normalizes a list of metrics to ensure uniqueness and reach targetCount
    /// using deterministic canonical order [.codex, .google, .workbuddy, .deepseek].
    public static func normalize(_ metrics: [WidgetMetricOption], targetCount: Int) -> [WidgetMetricOption] {
        let all = WidgetMetricOption.allCases
        var result: [WidgetMetricOption] = []
        for metric in metrics {
            if !result.contains(metric) {
                result.append(metric)
            } else {
                if let unused = all.first(where: { !result.contains($0) }) {
                    result.append(unused)
                }
            }
        }
        for candidate in all {
            if result.count >= targetCount { break }
            if !result.contains(candidate) {
                result.append(candidate)
            }
        }
        return Array(result.prefix(targetCount))
    }

    public func resolvedMetrics(for family: WidgetFamily) -> [WidgetMetricOption] {
        if family == .systemSmall {
            let p = primaryMetric ?? topLeft
            let s = secondaryMetric ?? topRight
            return Self.normalize([p, s], targetCount: 2)
        } else {
            let tl = primaryMetric ?? topLeft
            let tr = secondaryMetric ?? topRight
            return Self.normalize([tl, tr, bottomLeft, bottomRight], targetCount: 4)
        }
    }
}

