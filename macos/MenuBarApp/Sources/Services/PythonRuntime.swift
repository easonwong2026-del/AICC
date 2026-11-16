import Foundation

/// Which interpreter the bridge may run on.
///
/// The shipped App carries its own CPython 3.14 + OpenSSL 3.5 because the host's
/// `/usr/bin/python3` is 3.9 with LibreSSL, which cannot do TLS 1.3 at all. That
/// makes the bundled runtime a product requirement, not a convenience:
///
/// * a release build without it must fail loudly instead of quietly borrowing
///   whatever the user happens to have installed, because that changes whether
///   the LAN bridge works at all;
/// * only an explicit development launch (`AICC_DEV=1`) may point elsewhere.
///
/// Kept Foundation-only and side-effect free so it can be unit tested without
/// starting a process or touching the filesystem.
enum PythonRuntime {
    /// Where the runtime lives inside `AICC.app`.
    static let bundledRelativePath = "Contents/Resources/Python/bin/python3"

    enum Choice: Equatable {
        /// The runtime inside the bundle: the only production path.
        case bundled(String)
        /// An interpreter from the host, allowed only when `AICC_DEV=1`.
        case development(String)
        /// Nothing usable; the bridge must not start.
        case missing(String)

        var path: String? {
            switch self {
            case .bundled(let path), .development(let path): return path
            case .missing: return nil
            }
        }

        var message: String? {
            if case .missing(let reason) = self { return reason }
            return nil
        }
    }

    static func locate(
        bundleURL: URL,
        environment: [String: String],
        isExecutable: (String) -> Bool
    ) -> Choice {
        let bundled = bundleURL.appendingPathComponent(bundledRelativePath).path
        if isExecutable(bundled) {
            return .bundled(bundled)
        }

        guard environment["AICC_DEV"] == "1" else {
            return .missing(
                "内置 Python 运行时缺失：\(bundled)。正式包不会退回系统 Python，"
                + "请重新安装完整的 AICC 或运行 scripts/fetch-python-runtime.sh 后重新构建。"
            )
        }

        var candidates: [String] = []
        if let override = environment["AICC_PYTHON_PATH"], !override.isEmpty {
            candidates.append(override)
        }
        candidates.append(contentsOf: [
            "/opt/homebrew/bin/python3",
            "/usr/local/bin/python3",
            "/usr/bin/python3",
        ])
        if let found = candidates.first(where: isExecutable) {
            return .development(found)
        }
        return .missing("开发模式下也找不到可用的 python3")
    }
}
