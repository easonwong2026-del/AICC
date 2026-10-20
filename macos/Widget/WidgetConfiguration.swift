enum AICCWidgetMetadata {
    static let kind = "com.aieink.dashboard.menubar.widget"
    static let refreshURL = URL(string: "aicc-widget://refresh")!
}

import Foundation
import WidgetKit

/// What the widget draws.  Port, card count and the optional provider filter
/// live in the shared display-settings domain (`WidgetPreferences`), so one
/// sandboxed read is enough and no App Intent has to register to configure it.
struct AICCWidgetConfiguration: Codable, Equatable, Sendable {
    static func load() -> Self {
        let settings = WidgetPreferences.load()
        return Self(magpiePort: settings.port, cards: settings.cards, provider: settings.provider)
    }

    var magpiePort: Int
    var cards: WidgetCardDisplayMode
    var provider: String?

    init(magpiePort: Int = 3425, cards: WidgetCardDisplayMode = .automatic, provider: String? = nil) {
        self.magpiePort = magpiePort
        self.cards = cards
        self.provider = provider
    }

    enum CodingKeys: String, CodingKey {
        case magpiePort, cards, provider
        // Pre-3.0 configuration stored the same choice as `accounts`.
        case accounts
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        self.magpiePort = try container.decodeIfPresent(Int.self, forKey: .magpiePort) ?? 3425
        if let cards = try container.decodeIfPresent(WidgetCardDisplayMode.self, forKey: .cards) {
            self.cards = cards
        } else if let legacy = try container.decodeIfPresent(String.self, forKey: .accounts) {
            self.cards = WidgetCardDisplayMode.migrated(legacy)
        } else {
            self.cards = .automatic
        }
        let provider = try container.decodeIfPresent(String.self, forKey: .provider)
        self.provider = (provider?.isEmpty == false) ? provider : nil
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(magpiePort, forKey: .magpiePort)
        try container.encode(cards, forKey: .cards)
        try container.encodeIfPresent(provider, forKey: .provider)
    }

    /// Cards the widget may draw for a family, after the count preference.
    func capacity(familyLimit: Int) -> Int {
        cards.capacity(for: familyLimit)
    }
}
