import Foundation
import SwiftUI

class AppSettings: ObservableObject {
    @Published var launchAtLogin: Bool {
        didSet { UserDefaults.standard.set(launchAtLogin, forKey: "launchAtLogin") }
    }
    @Published var autoRefreshInterval: Double {
        didSet { UserDefaults.standard.set(autoRefreshInterval, forKey: "autoRefreshInterval") }
    }
    @Published var themeMode: String {
        didSet { UserDefaults.standard.set(themeMode, forKey: "themeMode") }
    }
    @Published var languageCode: String {
        didSet { UserDefaults.standard.set(languageCode, forKey: "languageCode") }
    }

    @Published var menuBarShowCodexStatus: Bool {
        didSet { UserDefaults.standard.set(menuBarShowCodexStatus, forKey: "menuBarShowCodexStatus") }
    }
    @Published var menuBarShowCodexBalance: Bool {
        didSet { UserDefaults.standard.set(menuBarShowCodexBalance, forKey: "menuBarShowCodexBalance") }
    }
    @Published var menuBarShowWorkBuddy: Bool {
        didSet { UserDefaults.standard.set(menuBarShowWorkBuddy, forKey: "menuBarShowWorkBuddy") }
    }
    @Published var menuBarShowDeepSeek: Bool {
        didSet { UserDefaults.standard.set(menuBarShowDeepSeek, forKey: "menuBarShowDeepSeek") }
    }
    @Published var menuBarShowOpenCodex: Bool {
        didSet { UserDefaults.standard.set(menuBarShowOpenCodex, forKey: "menuBarShowOpenCodex") }
    }

    @Published var ocxCustomPath: String {
        didSet { UserDefaults.standard.set(ocxCustomPath, forKey: "ocxCustomPath") }
    }

    @Published var debugMode: Bool {
        didSet { UserDefaults.standard.set(debugMode, forKey: "debugMode") }
    }

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
        switch AppThemeMode(rawValue: themeMode) ?? .system {
        case .light: return .light
        case .dark: return .dark
        case .system: return nil
        }
    }

    var language: AppLanguage {
        AppLanguage(rawValue: languageCode) ?? .system
    }

    var locale: Locale {
        language.locale
    }

    var presentationIdentity: SettingsPresentationIdentity {
        SettingsPresentationIdentity(languageCode: languageCode, themeMode: themeMode)
    }

    func localized(_ key: String) -> String {
        localized(key, languageCode: languageCode)
    }

    func localized(_ key: String, languageCode: String) -> String {
        let selectedLanguage = AppLanguage(rawValue: languageCode) ?? .system
        let localization = selectedLanguage.localizationIdentifier

        guard
            let path = Bundle.main.path(forResource: localization, ofType: "lproj"),
            let bundle = Bundle(path: path)
        else {
            return key
        }
        return bundle.localizedString(forKey: key, value: key, table: nil)
    }

    private init() {
        let defaults = UserDefaults.standard
        languageCode = defaults.string(forKey: "languageCode") ?? AppLanguage.system.rawValue
        let storedInterval = defaults.double(forKey: "autoRefreshInterval")
        autoRefreshInterval = storedInterval < 60 ? 120.0 : storedInterval
        if storedInterval < 60 {
            defaults.set(120.0, forKey: "autoRefreshInterval")
        }
        themeMode = defaults.string(forKey: "themeMode") ?? "system"
        launchAtLogin = defaults.object(forKey: "launchAtLogin") as? Bool ?? false
        menuBarShowCodexStatus = defaults.object(forKey: "menuBarShowCodexStatus") as? Bool ?? true
        menuBarShowCodexBalance = defaults.object(forKey: "menuBarShowCodexBalance") as? Bool ?? true
        menuBarShowWorkBuddy = defaults.object(forKey: "menuBarShowWorkBuddy") as? Bool ?? true
        menuBarShowDeepSeek = defaults.object(forKey: "menuBarShowDeepSeek") as? Bool ?? true
        menuBarShowOpenCodex = defaults.object(forKey: "menuBarShowOpenCodex") as? Bool ?? true
        ocxCustomPath = defaults.string(forKey: "ocxCustomPath") ?? ""
        debugMode = defaults.object(forKey: "debugMode") as? Bool ?? false
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
