import XCTest
@testable import AICCCore

/// 总览页的平台统计（PRD §3.3）。口径错了整个总览就是在骗人，所以单独守一份。
final class ConsoleFleetSummaryTests: XCTestCase {

    private func workbuddy(
        id: String,
        international: Bool = false,
        authStatus: String = "valid",
        tokenExpiresAt: String? = nil
    ) -> ConsoleAccount {
        ConsoleAccount(
            id: id,
            platform: "workbuddy",
            provider: international ? "workbuddy_ai" : "workbuddy",
            uid: "uid-\(id)",
            nickname: id,
            authStatus: authStatus,
            tokenExpiresAt: tokenExpiresAt
        )
    }

    private func qoderService(id: String, edition: String, usable: Bool) -> QoderServiceAccount {
        let authorizations: [[String: Any]] = usable
            ? [["source": "own_browser", "authStatus": "valid", "hasSecret": true]]
            : [["source": "magpie", "sourceKey": "magpie:1", "authStatus": "unknown", "hasSecret": false]]
        return QoderServiceAccount(json: [
            "id": id,
            "edition": edition,
            "uid": "uid-\(id)",
            "nickname": id,
            "authorizations": authorizations,
        ])!
    }

    private func profile(id: String, edition: String) -> QoderProfile {
        QoderProfile(json: [
            "profileId": id,
            "edition": edition,
            "displayName": id,
            "liveDir": "/tmp/live",
            "snapshotDir": "/tmp/snapshot",
            "authStatus": "valid",
            "runtime": ["state": "stopped"],
        ])!
    }

    func testCountsPerPlatformAndExcludesLocalProfiles() {
        let summary = ConsoleFleetSummary(
            accounts: [
                workbuddy(id: "cn-1"),
                workbuddy(id: "cn-2", authStatus: "expired"),
                workbuddy(id: "ai-1", international: true),
            ],
            qoderAccounts: [
                qoderService(id: "q-cn", edition: "qoder_cn", usable: true),
                qoderService(id: "q-intl", edition: "qoder_global", usable: false),
            ],
            qoderProfiles: [
                profile(id: "p-1", edition: "qoder_cn"),
                profile(id: "p-2", edition: "qoder_global"),
            ]
        )

        XCTAssertEqual(summary.serviceAccounts, 5)
        XCTAssertEqual(summary.healthy, 3)  // cn-1 + ai-1 + q-cn
        XCTAssertEqual(summary.issues, 2)   // cn-2 + q-intl
        // 本机 Profile 不计入服务账号，只单独计数。
        XCTAssertEqual(summary.localProfiles, 2)

        let cards = Dictionary(uniqueKeysWithValues: summary.cards.map { ($0.kind, $0) })
        XCTAssertEqual(cards[.workbuddyCN]?.accounts, 2)
        XCTAssertEqual(cards[.workbuddyCN]?.healthy, 1)
        XCTAssertEqual(cards[.workbuddyAI]?.accounts, 1)
        XCTAssertEqual(cards[.qoderCN]?.accounts, 1)
        XCTAssertEqual(cards[.qoderGlobal]?.healthy, 0)
    }

    /// Magpie 关联不算"授权正常"：AICC 手里没有凭据可验证。
    func testMagpieLinkedOnlyQoderAccountIsNotHealthy() {
        let summary = ConsoleFleetSummary(
            accounts: [],
            qoderAccounts: [qoderService(id: "q-cn", edition: "qoder_cn", usable: false)],
            qoderProfiles: []
        )
        XCTAssertEqual(summary.healthy, 0)
        XCTAssertEqual(summary.issues, 1)
    }

    /// 令牌过期的账号即使 authStatus 还是 valid 也要算异常。
    func testExpiredTokenCountsAsIssue() {
        let past = ISO8601DateFormatter().string(from: Date().addingTimeInterval(-3600))
        let summary = ConsoleFleetSummary(
            accounts: [workbuddy(id: "cn-1", tokenExpiresAt: past)],
            qoderAccounts: [],
            qoderProfiles: []
        )
        XCTAssertEqual(summary.issues, 1)
        XCTAssertEqual(summary.healthy, 0)
    }

    func testEmptyFleetHasNoIssues() {
        let summary = ConsoleFleetSummary(accounts: [], qoderAccounts: [], qoderProfiles: [])
        XCTAssertEqual(summary.serviceAccounts, 0)
        XCTAssertEqual(summary.issues, 0)
        XCTAssertEqual(summary.cards.count, 4)
    }

    /// 卡片点进去要能落到正确的筛选档位。
    func testPlatformKindFilters() {
        XCTAssertEqual(ConsolePlatformKind.workbuddyCN.filter, .workbuddy)
        XCTAssertEqual(ConsolePlatformKind.workbuddyAI.filter, .workbuddyAI)
        XCTAssertEqual(ConsolePlatformKind.qoderCN.filter, .qoder)
        XCTAssertEqual(ConsolePlatformKind.qoderGlobal.filter, .qoderGlobal)
    }

    /// QA 钩子用的选中项解析。
    func testAccountSelectionQAValueParsing() {
        XCTAssertEqual(ConsoleAccountSelection(qaValue: "workbuddy:acc-1"), .workbuddy("acc-1"))
        XCTAssertEqual(ConsoleAccountSelection(qaValue: "qoderService:q-1"), .qoderService("q-1"))
        XCTAssertEqual(ConsoleAccountSelection(qaValue: "qoderProfile:p-1"), .qoderProfile("p-1"))
        XCTAssertNil(ConsoleAccountSelection(qaValue: "unknown:x"))
        XCTAssertNil(ConsoleAccountSelection(qaValue: "workbuddy"))
    }
}
