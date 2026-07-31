import Foundation
import SwiftUI

enum AppLanguage: String, CaseIterable, Identifiable {
    case system
    case english = "en"
    case simplifiedChinese = "zh-Hans"

    var id: String { rawValue }

    var displayName: LocalizedStringKey {
        switch self {
        case .system: return "System Default"
        case .english: return "English"
        case .simplifiedChinese: return "Simplified Chinese"
        }
    }

    var locale: Locale {
        switch self {
        case .system: return .autoupdatingCurrent
        case .english: return Locale(identifier: "en")
        case .simplifiedChinese: return Locale(identifier: "zh-Hans")
        }
    }
}

class AppSettings: ObservableObject {
    @AppStorage("launchAtLogin") var launchAtLogin = false
    @AppStorage("autoRefreshInterval") var autoRefreshInterval = 120.0
    @AppStorage("themeMode") var themeMode = "system"
    @Published var languageCode: String {
        didSet {
            UserDefaults.standard.set(languageCode, forKey: "languageCode")
        }
    }

    @AppStorage("menuBarShowCodexStatus") var menuBarShowCodexStatus = true
    @AppStorage("menuBarShowCodexBalance") var menuBarShowCodexBalance = true
    @AppStorage("menuBarShowWorkBuddy") var menuBarShowWorkBuddy = true
    @AppStorage("menuBarShowDeepSeek") var menuBarShowDeepSeek = true
    @AppStorage("menuBarShowOpenCodex") var menuBarShowOpenCodex = true

    @AppStorage("ocxCustomPath") var ocxCustomPath = ""

    @AppStorage("debugMode") var debugMode = false

    // Dynamic provider collection (schema v1). Legacy per-provider toggles
    // are migrated once into these collections; no new per-provider switches
    // are added.
    @Published var providerOrder: [String] {
        didSet {
            UserDefaults.standard.set(
                ProviderPreferences.encodeProviderList(providerOrder),
                forKey: "providerOrderData"
            )
        }
    }

    @Published var hiddenProviders: Set<String> {
        didSet {
            UserDefaults.standard.set(
                ProviderPreferences.encodeProviderList(Array(hiddenProviders).sorted()),
                forKey: "hiddenProvidersData"
            )
        }
    }

    var preferredColorScheme: ColorScheme? {
        switch themeMode {
        case "light": return .light
        case "dark": return .dark
        default: return nil
        }
    }

    var language: AppLanguage {
        AppLanguage(rawValue: languageCode) ?? .system
    }

    var locale: Locale {
        language.locale
    }

    func localized(_ key: String) -> String {
        let localization: String
        switch language {
        case .system:
            localization = Locale.preferredLanguages.first?.hasPrefix("zh") == true ? "zh-Hans" : "en"
        case .english:
            localization = "en"
        case .simplifiedChinese:
            localization = "zh-Hans"
        }

        guard
            let path = Bundle.main.path(forResource: localization, ofType: "lproj"),
            let bundle = Bundle(path: path)
        else {
            return key
        }
        return bundle.localizedString(forKey: key, value: key, table: nil)
    }

    private init() {
        languageCode = UserDefaults.standard.string(forKey: "languageCode")
            ?? AppLanguage.system.rawValue
        let storedInterval = UserDefaults.standard.double(forKey: "autoRefreshInterval")
        if storedInterval < 60 {
            UserDefaults.standard.set(120.0, forKey: "autoRefreshInterval")
        }
        // Load or perform the one-time legacy migration. The migration result
        // is persisted inside loadOrMigrate, so initialization never depends
        // on property observers firing during init. didSet below still handles
        // every later user change.
        let providerPreferences = ProviderPreferences.loadOrMigrate(
            defaults: UserDefaults.standard
        )
        providerOrder = providerPreferences.order
        hiddenProviders = providerPreferences.hidden
    }

    static let shared = AppSettings()

    // MARK: - Dynamic provider settings

    func isProviderHidden(_ providerID: String) -> Bool {
        hiddenProviders.contains(providerID)
    }

    func setProviderHidden(_ providerID: String, hidden: Bool) {
        if hidden {
            hiddenProviders.insert(providerID)
        } else {
            hiddenProviders.remove(providerID)
        }
    }

    func moveProvider(_ providerID: String, direction: Int) {
        guard let index = providerOrder.firstIndex(of: providerID) else {
            // Unknown providers are appended; make the move relative to the
            // known prefix when possible.
            providerOrder.append(providerID)
            return
        }
        let target = index + direction
        guard target >= 0 && target < providerOrder.count else { return }
        providerOrder.swapAt(index, target)
    }

    func resetProviderOrder() {
        providerOrder = ProviderPreferences.defaultOrder
    }

    func appendUnknownProvider(_ providerID: String) {
        if !providerOrder.contains(providerID) {
            providerOrder.append(providerID)
        }
    }
}
