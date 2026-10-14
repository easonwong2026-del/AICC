import Foundation
import SwiftUI

class AppSettings: ObservableObject {
    @Published var bridgeEnabled: Bool {
        didSet { UserDefaults.standard.set(bridgeEnabled, forKey: "bridgeEnabled") }
    }
    @Published var magpiePort: Int {
        didSet { UserDefaults.standard.set(magpiePort, forKey: "magpiePort") }
    }
    @Published var themeMode: String {
        didSet { UserDefaults.standard.set(themeMode, forKey: "themeMode") }
    }
    @Published var languageCode: String {
        didSet { UserDefaults.standard.set(languageCode, forKey: "languageCode") }
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
    var language: AppLanguage { AppLanguage(rawValue: languageCode) ?? .system }
    var locale: Locale { language.locale }
    var presentationIdentity: SettingsPresentationIdentity {
        SettingsPresentationIdentity(languageCode: languageCode, themeMode: themeMode)
    }
    func localized(_ key: String) -> String { localized(key, languageCode: languageCode) }
    func localized(_ key: String, languageCode: String) -> String {
        let selected = AppLanguage(rawValue: languageCode) ?? .system
        guard let path = Bundle.main.path(forResource: selected.localizationIdentifier, ofType: "lproj"),
              let bundle = Bundle(path: path) else { return key }
        return bundle.localizedString(forKey: key, value: key, table: nil)
    }
    private init() {
        let defaults = UserDefaults.standard
        bridgeEnabled = defaults.bool(forKey: "bridgeEnabled")
        let port = defaults.integer(forKey: "magpiePort")
        magpiePort = (1...65535).contains(port) ? port : 3425
        themeMode = defaults.string(forKey: "themeMode") ?? "system"
        languageCode = defaults.string(forKey: "languageCode") ?? AppLanguage.system.rawValue
        debugMode = defaults.bool(forKey: "debugMode")
    }
    static let shared = AppSettings()
}
