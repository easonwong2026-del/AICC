import Foundation
import XCTest
@testable import AICCCore

final class ProviderModelsTests: XCTestCase {
    private func decode(_ json: String) throws -> ProvidersResponse {
        let data = Data(json.utf8)
        return try JSONDecoder().decode(ProvidersResponse.self, from: data)
    }

    // MARK: - Decoding tolerance

    func testUnknownJSONFieldsAreIgnored() throws {
        let response = try decode("""
        {
          "schema_version": 1,
          "updated_at": "2026-07-31 19:20:00",
          "future_field": {"anything": true},
          "providers": [
            {
              "id": "workbuddy",
              "display_name": "WorkBuddy",
              "category": "credits",
              "icon": "sparkles",
              "state": "connected",
              "available": true,
              "stale": false,
              "updated_at": "2026-07-31 19:19:50",
              "sort_order": 20,
              "future_capability": "ignored",
              "capabilities": ["refresh", "reconnect", "diagnostics"],
              "metrics": [
                {
                  "key": "points",
                  "label": "剩余积分",
                  "value": 5343.37,
                  "value_type": "number",
                  "format": "decimal",
                  "unit": "积分",
                  "primary": true,
                  "future_metric_field": 42
                }
              ],
              "actions": [
                {"id": "reconnect", "label": "重连 WorkBuddy", "kind": "reconnect", "local_only": true}
              ]
            }
          ]
        }
        """)
        XCTAssertEqual(response.schemaVersion, 1)
        XCTAssertEqual(response.providers.count, 1)
        let provider = try XCTUnwrap(response.providers.first)
        XCTAssertEqual(provider.id, "workbuddy")
        XCTAssertEqual(provider.displayName, "WorkBuddy")
        XCTAssertEqual(provider.sortOrder, 20)
        XCTAssertEqual(provider.metrics.count, 1)
        let metric = try XCTUnwrap(provider.metrics.first)
        XCTAssertEqual(metric.value, .number(5343.37))
        XCTAssertEqual(provider.actions.first?.kind, "reconnect")
        XCTAssertEqual(provider.actions.first?.actionID, "reconnect")
    }

    func testIntDoubleStringBoolAndNullValuesAllDecode() throws {
        let response = try decode("""
        {
          "schema_version": 1,
          "providers": [{
            "id": "mixed",
            "display_name": "Mixed",
            "state": "connected",
            "available": true,
            "metrics": [
              {"key": "int", "label": "Int", "value": 42, "value_type": "number", "format": "integer", "primary": true},
              {"key": "double", "label": "Double", "value": 12.5, "value_type": "number", "format": "decimal", "primary": false},
              {"key": "string", "label": "String", "value": "hello", "value_type": "text", "format": "plain", "primary": false},
              {"key": "bool", "label": "Bool", "value": true, "value_type": "status", "format": "plain", "primary": false},
              {"key": "null", "label": "Null", "value": null, "value_type": "number", "format": "decimal", "primary": false}
            ],
            "actions": []
          }]
        }
        """)
        let metrics = response.providers[0].metrics
        XCTAssertEqual(metrics[0].value, .number(42))
        XCTAssertEqual(metrics[1].value, .number(12.5))
        XCTAssertEqual(metrics[2].value, .text("hello"))
        XCTAssertEqual(metrics[3].value, .boolean(true))
        XCTAssertEqual(metrics[4].value, .null)
    }

    func testUnknownMetricTypeSafelyDegradesToText() throws {
        let response = try decode("""
        {
          "schema_version": 1,
          "providers": [{
            "id": "weird",
            "display_name": "Weird",
            "state": "connected",
            "available": true,
            "metrics": [
              {"key": "html", "label": "HTML", "value": "<b>x</b>", "value_type": "html", "format": "javascript", "primary": true}
            ],
            "actions": []
          }]
        }
        """)
        let metric = response.providers[0].metrics[0]
        XCTAssertEqual(metric.safeValueType, "text")
        XCTAssertEqual(metric.safeFormat, "plain")
        let display = MetricFormatter.format(value: metric.value, valueType: metric.safeValueType, format: metric.safeFormat, unit: metric.unit)
        XCTAssertFalse(display.placeholder)
        XCTAssertEqual(display.number, "<b>x</b>")
    }

    func testMalformedMetricValueDoesNotFailProviderDecode() throws {
        let response = try decode("""
        {
          "schema_version": 1,
          "providers": [{
            "id": "odd",
            "display_name": "Odd",
            "state": "connected",
            "available": true,
            "metrics": [
              {"key": "broken", "label": "Broken", "value": {"nested": true}, "value_type": "number", "format": "decimal", "primary": true}
            ],
            "actions": []
          }]
        }
        """)
        XCTAssertEqual(response.providers[0].metrics[0].value, .null)
    }

    func testEmptyProviderListDecodes() throws {
        let response = try decode(#"{"schema_version": 1, "providers": []}"#)
        XCTAssertTrue(response.providers.isEmpty)
    }

    func testProviderWithoutMetricsHasReasonableState() throws {
        let response = try decode("""
        {"schema_version": 1, "providers": [{"id": "empty", "display_name": "Empty", "state": "unavailable", "available": false, "metrics": [], "actions": []}]}
        """)
        let provider = response.providers[0]
        XCTAssertTrue(provider.metrics.isEmpty)
        XCTAssertTrue(provider.primaryMetrics.isEmpty)
        XCTAssertEqual(provider.state, "unavailable")
    }

    // MARK: - Primary metric selection

    func testPrimaryMetricSelectionAndOrder() throws {
        let response = try decode("""
        {
          "schema_version": 1,
          "providers": [{
            "id": "codex",
            "display_name": "Codex",
            "state": "connected",
            "available": true,
            "metrics": [
              {"key": "weekly_remaining", "label": "Weekly", "value": 92, "value_type": "percentage", "format": "percent", "unit": "%", "primary": true},
              {"key": "five_hour_remaining", "label": "5h", "value": 65, "value_type": "percentage", "format": "percent", "unit": "%", "primary": true},
              {"key": "weekly_reset", "label": "Reset", "value": "7月17日", "value_type": "text", "format": "plain", "primary": false}
            ],
            "actions": []
          }]
        }
        """)
        let provider = response.providers[0]
        XCTAssertEqual(provider.primaryMetrics.map(\.key), ["weekly_remaining", "five_hour_remaining"])
        XCTAssertEqual(provider.secondaryMetrics.map(\.key), ["weekly_reset"])
    }

    // MARK: - Number formatting

    func testLongNumbersFormatCorrectly() {
        XCTAssertEqual(MetricFormatter.decimalString(5000, maxFractionDigits: 2, minFractionDigits: 0, grouping: true), "5,000")
        XCTAssertEqual(MetricFormatter.decimalString(5343.37, maxFractionDigits: 2, minFractionDigits: 0, grouping: true), "5,343.37")
        XCTAssertEqual(MetricFormatter.decimalString(12_500, maxFractionDigits: 2, minFractionDigits: 0, grouping: true), "12,500")
        XCTAssertEqual(MetricFormatter.decimalString(999_999.99, maxFractionDigits: 2, minFractionDigits: 0, grouping: true), "999,999.99")
        XCTAssertEqual(MetricFormatter.decimalString(1_234_567, maxFractionDigits: 2, minFractionDigits: 0, grouping: true), "1,234,567")
    }

    func testLongNumberDropsDecimalsInsteadOfScientificNotation() {
        let display = MetricFormatter.format(value: .number(9_876_543_210.99), valueType: "number", format: "decimal", unit: "积分")
        XCTAssertEqual(display.number, "9,876,543,211")
        XCTAssertFalse(display.number?.contains("e") ?? true)
        XCTAssertFalse(display.number?.contains("E") ?? true)
    }

    func testPercentCurrencyAndUnitDisplay() {
        let percent = MetricFormatter.format(value: .number(92.4), valueType: "percentage", format: "percent", unit: "%")
        XCTAssertEqual(percent.number, "92")
        XCTAssertEqual(percent.unit, "%")

        let currency = MetricFormatter.format(value: .number(128.5), valueType: "currency", format: "currency", unit: "CNY")
        XCTAssertEqual(currency.number, "128.50")
        XCTAssertEqual(currency.unit, "CNY")

        let points = MetricFormatter.format(value: .number(5343.37), valueType: "number", format: "decimal", unit: "积分")
        XCTAssertEqual(points.number, "5,343.37")
        XCTAssertEqual(points.unit, "积分")
    }

    func testCompactFormatForVeryLongNumbers() {
        XCTAssertEqual(MetricFormatter.compactString(1_234_567_890), "1.23B")
        XCTAssertEqual(MetricFormatter.compactString(12_500_000), "12.5M")
        XCTAssertEqual(MetricFormatter.compactString(999), "999")
    }

    func testNullValueIsPlaceholder() {
        let display = MetricFormatter.format(value: .null, valueType: "number", format: "decimal", unit: "积分")
        XCTAssertTrue(display.placeholder)
        XCTAssertNil(display.number)
    }

    func testDurationFormatting() {
        XCTAssertEqual(MetricFormatter.durationString(40), "40s")
        XCTAssertEqual(MetricFormatter.durationString(400), "6m 40s")
        XCTAssertEqual(MetricFormatter.durationString(3900), "1h 05m")
    }

    // MARK: - Typography

    func testPrimaryFontNeverShrinksBelowReadableFloor() {
        XCTAssertEqual(DashboardTypography.primaryFontSize(number: "92"), DashboardTypography.primaryMetricLarge)
        XCTAssertEqual(DashboardTypography.primaryFontSize(number: "5,343.37"), 34)
        XCTAssertEqual(DashboardTypography.primaryFontSize(number: "999,999.99"), 32)
        XCTAssertGreaterThanOrEqual(DashboardTypography.primaryFontSize(number: "9,876,543,210"), 30)
        XCTAssertGreaterThanOrEqual(DashboardTypography.primaryFontSize(number: "9,876,543,210", compact: true), 24)
    }

    // MARK: - Preferences

    func testLegacySettingsMigrateToHiddenCollection() {
        let hidden = ProviderPreferences.hiddenAfterMigration(
            showCodexStatus: true,
            showWorkBuddy: false,
            showDeepSeek: true
        )
        XCTAssertEqual(hidden, ["workbuddy"])
    }

    // MARK: - Legacy visibility migration (0.1)

    private func makeDefaults() throws -> UserDefaults {
        let name = "AICCMigrationTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: name))
        defaults.removePersistentDomain(forName: name)
        addTeardownBlock { [weak defaults] in
            defaults?.removePersistentDomain(forName: name)
        }
        return defaults
    }

    func testMissingLegacyKeysNeverHideProvidersOnFreshInstall() throws {
        let defaults = try makeDefaults()
        XCTAssertTrue(ProviderPreferences.needsLegacyVisibilityMigration(defaults: defaults))
        let hidden = ProviderPreferences.hiddenAfterMigration(
            showCodexStatus: nil,
            showWorkBuddy: nil,
            showDeepSeek: nil
        )
        XCTAssertTrue(hidden.isEmpty)
        // The safe reader treats a missing key as the legacy default (true),
        // so a fresh install keeps every provider visible.
        XCTAssertTrue(ProviderPreferences.storedBool("menuBarShowCodexStatus", defaults: defaults, default: true))
        XCTAssertTrue(ProviderPreferences.storedBool("menuBarShowWorkBuddy", defaults: defaults, default: true))
        XCTAssertTrue(ProviderPreferences.storedBool("menuBarShowDeepSeek", defaults: defaults, default: true))
    }

    func testExplicitFalseHidesOnlyThatProvider() throws {
        let defaults = try makeDefaults()
        defaults.set(false, forKey: "menuBarShowWorkBuddy")
        let hidden = ProviderPreferences.hiddenAfterMigration(
            showCodexStatus: ProviderPreferences.storedBool("menuBarShowCodexStatus", defaults: defaults, default: true),
            showWorkBuddy: ProviderPreferences.storedBool("menuBarShowWorkBuddy", defaults: defaults, default: true),
            showDeepSeek: ProviderPreferences.storedBool("menuBarShowDeepSeek", defaults: defaults, default: true)
        )
        XCTAssertEqual(hidden, ["workbuddy"])
    }

    func testDynamicSettingsExistingSkipsMigration() throws {
        let defaults = try makeDefaults()
        XCTAssertTrue(ProviderPreferences.needsLegacyVisibilityMigration(defaults: defaults))
        defaults.set(ProviderPreferences.encodeProviderList(["codex"]), forKey: "providerOrderData")
        XCTAssertFalse(ProviderPreferences.needsLegacyVisibilityMigration(defaults: defaults))
    }

    func testMigrationResultSurvivesRestart() throws {
        let defaults = try makeDefaults()
        // Simulate the AppSettings first-run migration path.
        defaults.set(false, forKey: "menuBarShowWorkBuddy")
        let hidden = ProviderPreferences.hiddenAfterMigration(
            showCodexStatus: ProviderPreferences.storedBool("menuBarShowCodexStatus", defaults: defaults, default: true),
            showWorkBuddy: ProviderPreferences.storedBool("menuBarShowWorkBuddy", defaults: defaults, default: true),
            showDeepSeek: ProviderPreferences.storedBool("menuBarShowDeepSeek", defaults: defaults, default: true)
        )
        defaults.set(ProviderPreferences.encodeProviderList(["codex", "workbuddy", "deepseek", "system"]), forKey: "providerOrderData")
        defaults.set(ProviderPreferences.encodeProviderList(Array(hidden).sorted()), forKey: "hiddenProvidersData")

        // Restart: the migration gate is closed and the stored hidden set is
        // decoded back unchanged.
        XCTAssertFalse(ProviderPreferences.needsLegacyVisibilityMigration(defaults: defaults))
        let decodedHidden = Set(ProviderPreferences.decodeProviderList(defaults.data(forKey: "hiddenProvidersData")) ?? [])
        XCTAssertEqual(decodedHidden, ["workbuddy"])
        let decodedOrder = ProviderPreferences.decodeProviderList(defaults.data(forKey: "providerOrderData"))
        XCTAssertEqual(decodedOrder, ["codex", "workbuddy", "deepseek", "system"])
    }

    // MARK: - Action routing (0.2)

    func testActionRouteUsesKindNeverDisplayID() throws {
        let response = try decode("""
        {
          "schema_version": 1,
          "providers": [{
            "id": "workbuddy",
            "display_name": "WorkBuddy",
            "state": "connected",
            "available": true,
            "metrics": [],
            "actions": [
              {"id": "reconnect_workbuddy", "label": "Reconnect", "kind": "reconnect", "local_only": true}
            ]
          }]
        }
        """)
        let action = try XCTUnwrap(response.providers.first?.actions.first)
        XCTAssertEqual(action.actionID, "reconnect_workbuddy")
        XCTAssertEqual(action.kind, "reconnect")
        let route = ProviderAPI.actionPath(providerID: "workbuddy", kind: action.kind)
        XCTAssertEqual(route, "/api/providers/workbuddy/actions/reconnect")
        XCTAssertNotEqual(route, "/api/providers/workbuddy/actions/reconnect_workbuddy")
        XCTAssertEqual(ProviderAPI.refreshPath(providerID: "workbuddy"), "/api/providers/workbuddy/refresh")
        XCTAssertNil(ProviderAPI.actionPath(providerID: "workbuddy", kind: ""))
        XCTAssertNil(ProviderAPI.actionPath(providerID: "", kind: "reconnect"))
    }

    func testProviderOrderingPutsUnknownProvidersLast() {
        struct Stub { let id: String; let sort: Int }
        let providers = [
            Stub(id: "system", sort: 90),
            Stub(id: "example", sort: 200),
            Stub(id: "codex", sort: 10),
            Stub(id: "workbuddy", sort: 20),
        ]
        let ordered = ProviderPreferences.ordered(
            providers,
            order: ["codex", "workbuddy", "deepseek", "system"],
            id: { $0.id },
            manifestSortOrder: { $0.sort }
        )
        XCTAssertEqual(ordered.map(\.id), ["codex", "workbuddy", "system", "example"])
    }

    func testProviderOrderingFallsBackToManifestSortOrder() {
        struct Stub { let id: String; let sort: Int }
        let providers = [
            Stub(id: "b", sort: 30),
            Stub(id: "a", sort: 10),
        ]
        let ordered = ProviderPreferences.ordered(
            providers,
            order: [],
            id: { $0.id },
            manifestSortOrder: { $0.sort }
        )
        XCTAssertEqual(ordered.map(\.id), ["a", "b"])
    }

    func testDefaultProviderOrder() {
        XCTAssertEqual(ProviderPreferences.defaultOrder, ["codex", "workbuddy", "deepseek", "system"])
    }
}
