import Foundation

// The sandboxed Widget reads only this display-settings domain, never host credentials.
enum WidgetPreferences {
    static let domain = "com.aieink.dashboard.widget-settings"
    static let defaults = UserDefaults(suiteName: domain)!

    struct Display: Equatable {
        var port: Int
        var cards: WidgetCardDisplayMode
        var provider: String?
        var providers: [String]
    }

    static func load(from defaults: UserDefaults = defaults) -> Display {
        let values = defaults.dictionary(forKey: "display") ?? [:]
        let port = values["port"] as? Int ?? 3425
        // Pre-3.0 settings stored the same choice under `accounts`.
        let cards = WidgetCardDisplayMode(rawValue: values["cards"] as? String ?? "")
            ?? WidgetCardDisplayMode.migrated(values["accounts"] as? String ?? "")
        let provider = (values["provider"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
        return Display(port: (1...65535).contains(port) ? port : 3425,
                       cards: cards,
                       provider: (provider?.isEmpty == false) ? provider : nil,
                       providers: values["providers"] as? [String] ?? [])
    }

    static func save(port: Int, cards: WidgetCardDisplayMode, provider: String? = nil,
                     providers: [String] = [], to defaults: UserDefaults = defaults) {
        var values: [String: Any] = ["port": (1...65535).contains(port) ? port : 3425,
                                     "cards": cards.rawValue,
                                     "providers": providers]
        if let provider, !provider.isEmpty { values["provider"] = provider }
        defaults.set(values, forKey: "display")
    }
}
