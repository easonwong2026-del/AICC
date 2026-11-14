import XCTest
@testable import AICCCore

/// 额度行的三个不可妥协点：未知不是 0、单位不相加、unverified 要说清楚原因。
final class WorkerQuotaModelsTests: XCTestCase {
    private func row(_ json: [String: Any]) -> WorkerQuotaRow? {
        WorkerQuotaRow(json: json)
    }

    func testUnverifiedRowKeepsQuotaUnknown() {
        let parsed = row([
            "accountId": "qa_1",
            "provider": "qoder_cn",
            "status": "unverified",
            "nickname": "测试",
            "source": "own_browser",
            "error": "缺 client_id",
        ])
        XCTAssertEqual(parsed?.status, "unverified")
        XCTAssertNil(parsed?.total)
        XCTAssertNil(parsed?.remaining)
        // 未知额度不能渲染成 "0"。
        XCTAssertNil(parsed?.remainingText)
        XCTAssertEqual(parsed?.error, "缺 client_id")
        XCTAssertEqual(parsed?.providerName, "Qoder 国内版")
    }

    func testStringNumbersFromTheRealAPIAreParsed() {
        // 服务端返回的是字符串，且可能是小数。
        let parsed = row([
            "accountId": "a",
            "provider": "workbuddy_ai",
            "status": "ok",
            "total": "350",
            "remaining": "349.5",
            "used": "0.5",
            "unit": "credits",
        ])
        XCTAssertEqual(parsed?.total, 350)
        XCTAssertEqual(parsed?.remaining, 349.5)
        XCTAssertEqual(parsed?.totalText, "总额 350 credits")
        XCTAssertEqual(parsed?.remainingShare ?? -1, 349.5 / 350 * 100, accuracy: 0.001)
    }

    func testZeroTotalNeverDrawsAShare() {
        let parsed = row([
            "accountId": "a", "provider": "workbuddy", "status": "ok",
            "total": 0, "remaining": 0,
        ])
        XCTAssertNil(parsed?.remainingShare, "总额为 0 时不能画占比")
    }

    func testUsageCarriesWindowAndModels() {
        let parsed = row([
            "accountId": "a", "provider": "workbuddy_ai", "status": "ok",
            "usage": [
                "windowDays": 7,
                "requests": 12,
                "credits": 8.5,
                "truncated": true,
                "models": [["model": "hy4-free", "requests": 12, "credits": 8.5]],
            ],
        ])
        XCTAssertEqual(parsed?.usage?.windowDays, 7)
        XCTAssertEqual(parsed?.usage?.requests, 12)
        XCTAssertTrue(parsed?.usage?.truncated ?? false)
        XCTAssertEqual(parsed?.usage?.models.first?.model, "hy4-free")
    }

    func testMissingFieldsRejectTheRow() {
        XCTAssertNil(row(["provider": "workbuddy"]))
        XCTAssertNil(row(["accountId": "a", "provider": "workbuddy"]))
    }
}
