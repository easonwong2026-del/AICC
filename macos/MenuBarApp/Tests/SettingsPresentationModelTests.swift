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

    func testLanguagePickerUsesNativeNamesAndLocalizesOnlySystemDefault() {
        XCTAssertEqual(
            AppLanguage.english.pickerDisplayName(localize: { _ in "系统默认" }),
            "English"
        )
        XCTAssertEqual(
            AppLanguage.simplifiedChinese.pickerDisplayName(localize: { _ in "系统默认" }),
            "简体中文"
        )
        XCTAssertEqual(
            AppLanguage.system.pickerDisplayName(localize: { _ in "跟随系统" }),
            "跟随系统"
        )
        XCTAssertEqual(
            AppLanguage.system.pickerDisplayName(localize: { _ in "System Default" }),
            "System Default"
        )
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

    func testSettingsLifecycleCanBePresentedAgainAfterWindowClose() {
        var lifecycle = SettingsWindowLifecycle()
        XCTAssertTrue(lifecycle.beginPresentation())
        lifecycle.close()

        XCTAssertTrue(lifecycle.beginPresentation())
        XCTAssertEqual(lifecycle.state, .presented)
    }
}
