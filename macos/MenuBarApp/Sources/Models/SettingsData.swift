import Foundation
import SwiftUI

class AppSettings: ObservableObject {
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
    @Published var menuBarShowSystem: Bool {
        didSet { UserDefaults.standard.set(menuBarShowSystem, forKey: "menuBarShowSystem") }
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
        menuBarShowCodexStatus = defaults.object(forKey: "menuBarShowCodexStatus") as? Bool ?? true
        menuBarShowCodexBalance = defaults.object(forKey: "menuBarShowCodexBalance") as? Bool ?? true
        menuBarShowWorkBuddy = defaults.object(forKey: "menuBarShowWorkBuddy") as? Bool ?? true
        menuBarShowDeepSeek = defaults.object(forKey: "menuBarShowDeepSeek") as? Bool ?? true
        menuBarShowSystem = defaults.object(forKey: "menuBarShowSystem") as? Bool ?? true
        menuBarShowOpenCodex = defaults.object(forKey: "menuBarShowOpenCodex") as? Bool ?? true
        ocxCustomPath = defaults.string(forKey: "ocxCustomPath") ?? ""
        debugMode = defaults.object(forKey: "debugMode") as? Bool ?? false
    }

    static let shared = AppSettings()
}
