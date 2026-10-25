enum AICCWidgetMetadata {
    static let kind = "com.aieink.dashboard.menubar.widget"
    /// The usage board is a separate widget so both can sit on the desktop.
    static let boardKind = "com.aieink.dashboard.menubar.board"
    static let refreshURL = URL(string: "aicc-widget://refresh")!

    /// Footer taps travel as links.  `Button(intent:)` needs App Intents
    /// metadata that only a signed Xcode build can produce, and a hand-written
    /// copy is silently ignored by the system, so the buttons use the one
    /// interaction WidgetKit has always honoured: open the app with a URL.
    static func boardStepURL(_ step: Int) -> URL {
        URL(string: "aicc-widget://board/\(step > 0 ? "next" : "previous")")!
    }

    /// The only two paths the host app acts on; anything else is ignored rather
    /// than guessed at, so a typo cannot silently page backwards out of a cold
    /// launch.
    static func boardStep(fromPath path: String) -> Int? {
        switch path {
        case "/next": return 1
        case "/previous": return -1
        default: return nil
        }
    }
}

import Foundation
import WidgetKit

/// What the widget draws.  Port, card count and the optional provider filter
/// live in the shared display-settings domain (`WidgetPreferences`), so one
/// sandboxed read is enough and no App Intent has to register to configure it.
struct AICCWidgetConfiguration: Codable, Equatable, Sendable {
    static func load() -> Self {
        let settings = WidgetPreferences.load()
        return Self(magpiePort: settings.port, cards: settings.cards,
                    provider: settings.provider, selection: settings.selection)
    }

    var magpiePort: Int
    var cards: WidgetCardDisplayMode
    var provider: String?
    /// Windows the user ticked in settings, in display order.  Empty means
    /// "decide for me".
    var selection: [String]

    init(magpiePort: Int = 3425, cards: WidgetCardDisplayMode = .automatic,
         provider: String? = nil, selection: [String] = []) {
        self.magpiePort = magpiePort
        self.cards = cards
        self.provider = provider
        self.selection = selection
    }

    enum CodingKeys: String, CodingKey {
        case magpiePort, cards, provider, selection
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
        self.selection = try container.decodeIfPresent([String].self, forKey: .selection) ?? []
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(magpiePort, forKey: .magpiePort)
        try container.encode(cards, forKey: .cards)
        try container.encodeIfPresent(provider, forKey: .provider)
        try container.encode(selection, forKey: .selection)
    }

    /// Cards the widget may draw for a family, after the count preference.
    func capacity(familyLimit: Int) -> Int {
        cards.capacity(for: familyLimit)
    }
}
