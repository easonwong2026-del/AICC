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

/// PR-B: the TLS identity the LAN bridge serves, as reported by the worker.
struct BridgeTlsStatus: Equatable, Sendable {
    let spki: String?
    let notAfter: String?
    let generation: Int?
    let caCreated: Bool?
    let leafReissued: Bool?

    static func parse(_ payload: [String: Any]) -> BridgeTlsStatus {
        BridgeTlsStatus(
            spki: payload["spki"] as? String,
            notAfter: payload["notAfter"] as? String,
            generation: payload["generation"] as? Int,
            caCreated: payload["caCreated"] as? Bool,
            leafReissued: payload["leafReissued"] as? Bool
        )
    }

    /// Grouped in fours, so it can be compared with the e-ink screen.
    var groupedFingerprint: String {
        guard let spki else { return "" }
        return stride(from: 0, to: spki.count, by: 4).map { offset -> String in
            let start = spki.index(spki.startIndex, offsetBy: offset)
            let end = spki.index(start, offsetBy: min(4, spki.count - offset))
            return String(spki[start..<end])
        }.joined(separator: " ")
    }
}
