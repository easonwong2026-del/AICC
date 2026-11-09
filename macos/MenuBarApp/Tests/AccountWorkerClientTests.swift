import XCTest
@testable import AICCCore

/// 直接拿真进程验证 `AccountWorkerClient` 的三条命脉：
/// 正常往返、超时唤醒、助手异常退出时唤醒**所有**在途请求。
///
/// 用 `/bin/sh` 脚本当假助手，所以不依赖 Rust 产物，CI 上也能跑。
final class AccountWorkerClientTests: XCTestCase {
    private var tempDirs: [URL] = []

    override func tearDown() {
        for dir in tempDirs {
            try? FileManager.default.removeItem(at: dir)
        }
        tempDirs.removeAll()
        super.tearDown()
    }

    /// 生成一个可执行的假助手脚本。
    private func makeWorker(_ body: String) throws -> URL {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("aicc-worker-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        tempDirs.append(dir)

        let url = dir.appendingPathComponent("worker.sh")
        try ("#!/bin/sh\n" + body).write(to: url, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: url.path)
        return url
    }

    /// 读一行、回一行，把请求 id 原样带回——和真助手的 JSONL 行为一致。
    private let echoWorker = """
    while IFS= read -r line; do
      id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\\1/')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"pong":true}}\\n' "$id"
    done
    """

    func testCallResolvesWithResult() async throws {
        let client = AccountWorkerClient(executable: try makeWorker(echoWorker))
        try client.start()
        defer { client.stop() }

        let result = try await client.call("ping", timeout: 5)
        XCTAssertEqual(result["pong"] as? Bool, true)
    }

    func testTimeoutWakesTheContinuation() async throws {
        // 永不回包，但进程活着：只能靠超时把调用方唤醒。
        let client = AccountWorkerClient(executable: try makeWorker("sleep 30\n"))
        try client.start()
        defer { client.stop() }

        let started = Date()
        do {
            _ = try await client.call("ping", timeout: 0.4)
            XCTFail("超时后不应该拿到结果")
        } catch AccountWorkerFailure.timeout {
            // 期望路径
        } catch {
            XCTFail("错误类型不对：\(error)")
        }
        XCTAssertLessThan(Date().timeIntervalSince(started), 5, "超时必须及时唤醒，不能一直挂着")
    }

    func testAbnormalExitWakesEveryPendingCall() async throws {
        // 先活着让请求写进管道，再异常退出。
        let client = AccountWorkerClient(executable: try makeWorker("sleep 0.4\nexit 3\n"))
        try client.start()
        defer { client.stop() }

        try await withThrowingTaskGroup(of: Void.self) { group in
            for _ in 0..<3 {
                group.addTask {
                    do {
                        _ = try await client.call("ping", timeout: 10)
                        XCTFail("助手退出后不应返回结果")
                    } catch let AccountWorkerFailure.transport(detail) {
                        XCTAssertFalse(detail.isEmpty, "应带上退出原因")
                    } catch {
                        XCTFail("错误类型不对：\(error)")
                    }
                }
            }
            try await group.waitForAll()
        }
    }

    func testCallAfterStopFailsFast() async throws {
        let client = AccountWorkerClient(executable: try makeWorker(echoWorker))
        try client.start()
        _ = try await client.call("ping", timeout: 5)

        client.stop()

        let started = Date()
        do {
            _ = try await client.call("ping", timeout: 10)
            XCTFail("停止后不应还能调用")
        } catch AccountWorkerFailure.notRunning {
            // 期望路径
        } catch {
            XCTFail("错误类型不对：\(error)")
        }
        XCTAssertLessThan(Date().timeIntervalSince(started), 5, "停止后必须立刻失败")
    }

    func testStartFailsForMissingExecutable() {
        let client = AccountWorkerClient(executable: URL(fileURLWithPath: "/nonexistent/aicc-worker"))
        XCTAssertThrowsError(try client.start())
    }
}
