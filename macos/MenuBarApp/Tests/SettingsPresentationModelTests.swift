import XCTest
@testable import AICCCore

final class SettingsPresentationModelTests: XCTestCase {
    func testLanguageAndThemeIdentityChangesOnlyWhenPresentationChanges() {
        let first = SettingsPresentationIdentity(languageCode: "en", themeMode: "dark")
        let same = SettingsPresentationIdentity(languageCode: "en", themeMode: "dark")
        let different = SettingsPresentationIdentity(languageCode: "zh-Hans", themeMode: "dark")

        XCTAssertEqual(first, same)
        XCTAssertNotEqual(first, different)
    }

    func testLocaleSelectionHasStableExplicitIdentifiers() {
        XCTAssertEqual(AppLanguage.system.localizationIdentifier, AppLanguage.system.localizationIdentifier)
        XCTAssertEqual(AppLanguage.english.localizationIdentifier, "en")
        XCTAssertEqual(AppLanguage.simplifiedChinese.localizationIdentifier, "zh-Hans")
        XCTAssertEqual((AppLanguage(rawValue: "invalid") ?? .system).localizationIdentifier, AppLanguage.system.localizationIdentifier)
    }

    func testThemeSelectionHasThreeSupportedModes() {
        XCTAssertEqual(AppThemeMode.allCases, [.system, .light, .dark])
        XCTAssertNil(AppThemeMode(rawValue: "invalid"))
    }

    func testStatusDisplayStateRemovesDuplicateUpdates() {
        let first = StatusItemDisplayState(
            remaining: 77,
            showStatus: true,
            showBalance: true,
            languageCode: "en",
            themeMode: "system"
        )
        let same = StatusItemDisplayState(
            remaining: 77,
            showStatus: true,
            showBalance: true,
            languageCode: "en",
            themeMode: "system"
        )
        XCTAssertEqual(first, same)
    }

    func testSettingsLifecycleReturnsToClosedAfterClose() {
        var lifecycle = SettingsWindowLifecycle()
        XCTAssertEqual(lifecycle.state, .closed)
        XCTAssertTrue(lifecycle.beginPresentation())
        XCTAssertEqual(lifecycle.state, .presented)
        XCTAssertFalse(lifecycle.beginPresentation())
        lifecycle.close()
        XCTAssertEqual(lifecycle.state, .closed)
    }
}
