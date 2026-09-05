import XCTest
@testable import AICCCore

final class AppSettingsTests: XCTestCase {
    func testMenuBarShowSystemDefaultsAndPersists() {
        let defaults = UserDefaults.standard
        let settingKey = "menuBarShowSystem"
        let autoRefreshKey = "autoRefreshInterval"
        let previousSetting = defaults.object(forKey: settingKey)
        let previousAutoRefresh = defaults.object(forKey: autoRefreshKey)

        defaults.removeObject(forKey: settingKey)
        if previousAutoRefresh == nil {
            defaults.set(120.0, forKey: autoRefreshKey)
        }

        let settings = AppSettings.shared
        defer {
            if let previousSetting {
                defaults.set(previousSetting, forKey: settingKey)
                if let value = previousSetting as? Bool {
                    settings.menuBarShowSystem = value
                }
            } else {
                settings.menuBarShowSystem = true
                defaults.removeObject(forKey: settingKey)
            }

            if let previousAutoRefresh {
                defaults.set(previousAutoRefresh, forKey: autoRefreshKey)
            } else {
                defaults.removeObject(forKey: autoRefreshKey)
            }
        }

        XCTAssertTrue(settings.menuBarShowSystem)

        settings.menuBarShowSystem = false
        XCTAssertEqual(defaults.object(forKey: settingKey) as? Bool, false)

        settings.menuBarShowSystem = true
        XCTAssertTrue(defaults.bool(forKey: settingKey))
    }

    func testMenuBarShowGoogleQuotaDefaultsAndPersists() {
        let defaults = UserDefaults.standard
        let settingKey = "menuBarShowGoogleQuota"
        let previous = defaults.object(forKey: settingKey)
        defaults.removeObject(forKey: settingKey)
        let settings = AppSettings.shared

        defer {
            if let previous {
                defaults.set(previous, forKey: settingKey)
                settings.menuBarShowGoogleQuota = (previous as? Bool) ?? true
            } else {
                settings.menuBarShowGoogleQuota = true
                defaults.removeObject(forKey: settingKey)
            }
        }

        XCTAssertTrue(settings.menuBarShowGoogleQuota)
        settings.menuBarShowGoogleQuota = false
        XCTAssertEqual(defaults.object(forKey: settingKey) as? Bool, false)
        settings.menuBarShowGoogleQuota = true
        XCTAssertTrue(defaults.bool(forKey: settingKey))
    }
}
