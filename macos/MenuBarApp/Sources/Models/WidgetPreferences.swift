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
        var selection: [String]
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
                       providers: values["providers"] as? [String] ?? [],
                       selection: values["selection"] as? [String] ?? [])
    }

    static func save(port: Int, cards: WidgetCardDisplayMode, provider: String? = nil,
                     providers: [String] = [], selection: [String] = [],
                     to defaults: UserDefaults = defaults) {
        var values: [String: Any] = ["port": (1...65535).contains(port) ? port : 3425,
                                     "cards": cards.rawValue,
                                     "providers": providers,
                                     "selection": selection]
        if let provider, !provider.isEmpty { values["provider"] = provider }
        defaults.set(values, forKey: "display")
    }

    /// A page turn requested from outside the extension.
    ///
    /// The widget's own container is not writable from the host app, so a footer
    /// tap has to travel this way: the app drops a request into the one domain
    /// the widget may read, and the extension applies it on its next timeline
    /// build.  `at` makes the request idempotent — the widget remembers the last
    /// stamp it honoured, so a rebuild cannot apply the same tap twice.
    struct BoardCommand: Equatable {
        var step: Int
        var at: Double
    }

    private static let boardCommandKey = "boardCommand"

    static func boardCommand(from defaults: UserDefaults = defaults) -> BoardCommand? {
        guard let values = defaults.dictionary(forKey: boardCommandKey),
              let step = values["step"] as? Int,
              let at = values["at"] as? Double else { return nil }
        return BoardCommand(step: step, at: at)
    }

    static func requestBoardStep(_ step: Int, at: Date = .now,
                                 to defaults: UserDefaults = defaults) {
        // One page per request; the provider multiplies nothing.
        let clamped = step < 0 ? -1 : 1
        defaults.set(["step": clamped, "at": at.timeIntervalSince1970], forKey: boardCommandKey)
    }
}
