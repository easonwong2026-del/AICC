import Foundation
import XCTest
@testable import AICCCore

/// The board's numbers come from three Magpie payloads; these tests pin the
/// derivations to the ones `metrics.py` in magpie-usage-display uses.
final class UsageModelTests: XCTestCase {
    private func data(_ object: [String: Any]) -> Data {
        try! JSONSerialization.data(withJSONObject: object)
    }

    private var usage: Data {
        data([
            "calls": 6509, "errors": 111, "input": 59_016_319, "output": 2_521_394,
            "cache_read": 944_403_004, "cache_write": 0, "reasoning": 809_870,
            "cost": 156.6481293300004, "sessionCount": 93,
            "series": [
                ["calls": 100, "errors": 1, "input": 10, "output": 5, "cache_read": 0, "reasoning": 1,
                 "timed": 90, "ttft_ms": 90_000, "cost": 1],
                ["calls": 965, "errors": 15, "input": 5_113_475, "output": 446_515,
                 "cache_read": 216_485_587, "cache_write": 0, "reasoning": 164_521,
                 "timed": 950, "ttft_ms": 16_420_143, "cost": 14.91],
            ],
            "models": [
                ["name": "gpt-5.6-luna", "calls": 905],
                ["name": "deepseek-v4.1-flash", "calls": 1_200],
            ],
        ])
    }

    private var requests: Data {
        data(["rows": [
            ["served": "deepseek-v4.1-flash", "status": 200, "ms": 12_400],
            ["model": "gpt-5.6-luna", "status": 429, "ms": 900],
        ]])
    }

    private var quotas: Data {
        data(["data": [
            ["provider_name": "Codex", "windows": [
                ["name": "5 hours", "remaining": 95],
                ["name": "7 days", "remaining": 64, "resetsAt": "2026-10-27T10:00:00+08:00"],
            ]],
            ["provider_name": "WorkBuddy", "windows": [
                ["name": "Credits", "remaining": 99, "unlimited": false],
                ["name": "Lifetime", "unlimited": true],
            ]],
        ]])
    }

    func testSuccessAndTokensComeFromTheLastDayBucket() {
        let snapshot = UsageParser.snapshot(usage: usage, requests: nil, quotas: nil)
        XCTAssertEqual(snapshot.callsToday, 965)
        XCTAssertEqual(snapshot.errorsToday, 15)
        XCTAssertEqual(snapshot.successPermille, 984)
        XCTAssertEqual(snapshot.tokensToday, 5_113_475 + 446_515 + 164_521 + 216_485_587)
        XCTAssertEqual(snapshot.averageTTFTMillis, 17_284)
        XCTAssertTrue(snapshot.available)
    }

    func testTotalsAndCacheHitMirrorTheCompanion() {
        let snapshot = UsageParser.snapshot(usage: usage, requests: nil, quotas: nil)
        XCTAssertEqual(snapshot.sessionCount, 93)
        XCTAssertEqual(snapshot.totalCalls, 6_509)
        XCTAssertEqual(snapshot.costMicros, 156_648_129)
        XCTAssertEqual(snapshot.cacheHitPermille, 941)
        XCTAssertEqual(snapshot.topModel, "deepseek-v4.1-flash")
        XCTAssertEqual(snapshot.topModelCalls, 1_200)
    }

    func testLatestRequestCarriesStatusAndLatency() {
        let snapshot = UsageParser.snapshot(usage: nil, requests: requests, quotas: nil)
        XCTAssertEqual(snapshot.latestModel, "deepseek-v4.1-flash")
        XCTAssertEqual(snapshot.latestStatus, 200)
        XCTAssertEqual(snapshot.latestLatencyMillis, 12_400)
        XCTAssertFalse(snapshot.latestFailed)
        XCTAssertEqual(snapshot.recent.count, 2)
        XCTAssertTrue(snapshot.recent[1].status == 429)
    }

    func testTheTightestWindowIsPickedAcrossProviders() {
        let now = ISO8601DateFormatter().date(from: "2026-10-21T10:00:00Z")!
        let snapshot = UsageParser.snapshot(usage: nil, requests: nil, quotas: quotas, now: now)
        XCTAssertEqual(snapshot.remainingPercent, 64)
        XCTAssertEqual(snapshot.remainingWindow, "Codex · 7 days")
        XCTAssertEqual(snapshot.resetSeconds, 5 * 86400 + 18 * 3600, accuracy: 60)
    }

    func testAnEmptyPayloadIsUnavailableRatherThanZero() {
        let snapshot = UsageParser.snapshot(usage: nil, requests: nil, quotas: nil)
        XCTAssertFalse(snapshot.available)
        XCTAssertEqual(snapshot.callsToday, 0)
        XCTAssertEqual(snapshot.remainingPercent, 100)
    }

    func testFormattersMatchTheDevice() {
        XCTAssertEqual(UsageFormat.count(965), "965")
        XCTAssertEqual(UsageFormat.count(12_600), "12.6K")
        XCTAssertEqual(UsageFormat.count(944_403_004), "944M")
        XCTAssertEqual(UsageFormat.grouped(6_509), "6 509")
        XCTAssertEqual(UsageFormat.money(micros: 156_648_129), "$156.65")
        XCTAssertEqual(UsageFormat.seconds(millis: 12_400), "12s")
        XCTAssertEqual(UsageFormat.seconds(millis: 1_800), "1.8s")
        XCTAssertEqual(UsageFormat.seconds(millis: 900), "900ms")
        XCTAssertEqual(UsageFormat.tenths(985), "98.5")
        XCTAssertEqual(UsageFormat.tenths(1_000), "100")
        XCTAssertEqual(UsageFormat.countdown(seconds: 5 * 86400 + 22 * 3600), "5 天 22 时")
        XCTAssertEqual(UsageFormat.countdown(seconds: 4 * 3600 + 12 * 60), "4 时 12 分")
        XCTAssertEqual(UsageFormat.countdown(seconds: 0), "—")
    }
}
