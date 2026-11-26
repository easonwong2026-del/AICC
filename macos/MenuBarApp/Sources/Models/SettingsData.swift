import Foundation
import SwiftUI

class AppSettings: ObservableObject {
    @Published var bridgeEnabled: Bool {
        didSet { UserDefaults.standard.set(bridgeEnabled, forKey: "bridgeEnabled") }
    }
    /// 允许局域网设备连桥接。**默认关**：打开它会把额度数据暴露给同网段。
    ///
    /// 升级语义：老版本没有这个键，`bool(forKey:)` 返回 false，所以已经开着桥接的
    /// 用户升级后也只会回到"仅本机"，必须自己再点一次 —— 不能静默授予新的网络权限。
    @Published var bridgeLANEnabled: Bool {
        didSet { UserDefaults.standard.set(bridgeLANEnabled, forKey: "bridgeLANEnabled") }
    }
    @Published var magpiePort: Int {
        didSet {
            UserDefaults.standard.set(magpiePort, forKey: "magpiePort")
            publishWidgetSettings()
        }
    }
    @Published var widgetCards: WidgetCardDisplayMode {
        didSet {
            UserDefaults.standard.set(widgetCards.rawValue, forKey: "widgetCards")
            publishWidgetSettings()
        }
    }
    /// Empty means every provider Magpie reports.
    @Published var widgetProvider: String {
        didSet {
            UserDefaults.standard.set(widgetProvider, forKey: "widgetProvider")
            publishWidgetSettings()
        }
    }
    /// Quota windows the widgets draw, in display order.  Empty means automatic.
    @Published var widgetSelection: [String] {
        didSet {
            UserDefaults.standard.set(widgetSelection, forKey: "widgetSelection")
            publishWidgetSettings()
        }
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
    /// 启动后在后台静默检查更新（每 24 小时最多一次，不弹窗打扰）。默认开。
    @Published var autoCheckUpdates: Bool {
        didSet { UserDefaults.standard.set(autoCheckUpdates, forKey: "autoCheckUpdates") }
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
    /// Port, card count and provider filter live in the widget's own settings
    /// domain, so the extension never needs the host's preferences file.
    func publishWidgetSettings(providers: [String]? = nil) {
        WidgetPreferences.save(port: magpiePort, cards: widgetCards,
                               provider: widgetProvider.isEmpty ? nil : widgetProvider,
                               providers: providers ?? WidgetPreferences.load().providers,
                               selection: widgetSelection)
    }
    private init() {
        let defaults = UserDefaults.standard
        bridgeEnabled = defaults.bool(forKey: "bridgeEnabled")
        // 不存在的键 → false。老安装升级后不会自动获得局域网访问权限。
        bridgeLANEnabled = defaults.bool(forKey: "bridgeLANEnabled")
        let port = defaults.integer(forKey: "magpiePort")
        magpiePort = (1...65535).contains(port) ? port : 3425
        // `widgetAccounts` is the pre-3.0 key for the same choice.
        let stored = defaults.string(forKey: "widgetCards") ?? defaults.string(forKey: "widgetAccounts") ?? ""
        widgetCards = WidgetCardDisplayMode(rawValue: stored) ?? WidgetCardDisplayMode.migrated(stored)
        widgetProvider = defaults.string(forKey: "widgetProvider") ?? ""
        widgetSelection = defaults.stringArray(forKey: "widgetSelection") ?? []
        themeMode = defaults.string(forKey: "themeMode") ?? "system"
        languageCode = defaults.string(forKey: "languageCode") ?? AppLanguage.system.rawValue
        debugMode = defaults.bool(forKey: "debugMode")
        autoCheckUpdates = defaults.object(forKey: "autoCheckUpdates") as? Bool ?? true
        publishWidgetSettings()
    }
    static let shared = AppSettings()
}
