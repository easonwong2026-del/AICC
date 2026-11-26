import Combine
import Foundation
import SwiftUI
import AppKit
import WidgetKit

// MARK: - Server Manager

enum ServerHealthState: String {
    case stopped
    case starting
    case healthy
    case degraded
    case recovering
    case failed
}

enum ServerOwnership {
    case none
    case managed
    case external
}

enum ServerStopReason {
    case none
    case recovery
    case user
    case appExit
}

@MainActor
final class ServerManager: ObservableObject {
    static let shared = ServerManager()
    private static let productPort = 8765

    @Published var isServerRunning = false
    @Published var healthState: ServerHealthState = .stopped
    /// Set when the bundled runtime is missing, so the UI can say why the
    /// bridge refuses to start instead of just looking broken.
    @Published var runtimeError: String?
    /// Whether the LAN listener is really up. "LAN enabled" in Settings is only
    /// the user's intent; without a certificate the bridge keeps loopback only
    /// and this stays false.
    @Published var lanServing = false
    private var serverProcess: Process?
    private var ownership: ServerOwnership = .none
    private var supervisorTask: Task<Void, Never>?
    private var stopReason: ServerStopReason = .none
    private var restartFailures = 0
    private var failureWindowStarted = Date.distantPast
    private var nextRetryDelay: UInt64 = 1_000_000_000

    func resolveServerRoot() -> URL? {
        if let marker = Bundle.main.url(forResource: "ServerRoot", withExtension: "txt") {
            if let path = try? String(contentsOf: marker, encoding: .utf8)
                .trimmingCharacters(in: .whitespacesAndNewlines) {
                if path.hasPrefix("@resources/") {
                    let relative = String(path.dropFirst("@resources/".count))
                    return Bundle.main.resourceURL?.appendingPathComponent(relative, isDirectory: true)
                }
                if !path.isEmpty {
                    return URL(fileURLWithPath: path, isDirectory: true)
                }
            }
        }
        // Fallback: assume running from project root
        return Bundle.main.bundleURL
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
    }

    func startServer() async -> Bool {
        if serverProcess?.isRunning == true {
            let identity = await checkServerIdentity()
            return identity.alive && identity.compatible
        }

        stopReason = .none
        healthState = .starting
        // 局域网要开就必须先有证书：Python 侧没有证书时只开回环，不会退回明文。
        // 启动顺序因此是"先准备身份，再拉进程"，而不是让用户去猜为什么连不上。
        if AppSettings.shared.bridgeLANEnabled {
            _ = await AccountWorkerManager.shared.ensureBridgeTls()
        }
        guard let root = resolveServerRoot() else {
            healthState = .degraded
            return false
        }
        let serverScript = root.appendingPathComponent("server.py").path

        guard FileManager.default.fileExists(atPath: serverScript) else {
            healthState = .degraded
            return false
        }

        let identity = await checkServerIdentity()
        if identity.alive {
            if identity.compatible && !identity.isOrphan {
                isServerRunning = true
                ownership = .external
                healthState = .healthy
                return true
            } else {
                // Stale, incompatible, or orphaned server! Reclaim port.
                await reclaimPort(pid: identity.pid, trusted: identity.trusted)
            }
        }

        guard let python = resolvePython() else {
            healthState = .degraded
            return false
        }

        let process = Process()
        process.launchPath = python
        process.arguments = ["-B", serverScript]
        process.currentDirectoryPath = root.path

        var env = ProcessInfo.processInfo.environment
        var path = env["PATH"] ?? ""
        path = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:\(path)"
        env["PATH"] = path
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        env["EINK_ACCESS_LOG"] = AppSettings.shared.debugMode ? "1" : "0"
        // The shipped App uses the fixed product port; EINK_PORT remains for direct/dev server runs.
        env.removeValue(forKey: "EINK_PORT")
        // 监听范围由设置决定，不继承外部环境：默认只听回环，局域网与设备发现
        // 都必须由用户在设置里明确打开（关闭时连 UDP 发现一起停）。
        let lanAccess = AppSettings.shared.bridgeLANEnabled
        env["EINK_HOST"] = lanAccess ? "0.0.0.0" : "127.0.0.1"
        env["EINK_DISCOVERY"] = lanAccess ? "1" : "0"
        // No certificate, no listener: the bridge refuses to fall back to
        // plaintext, so the identity has to exist before it starts.
        if lanAccess {
            env["EINK_CERT_DIR"] = AICCPaths.certDirectory.path
        }
        let hostBuild = (Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !hostBuild.isEmpty {
            env["AICC_BUILD"] = hostBuild
        }
        env["AICC_PARENT_PID"] = "\(ProcessInfo.processInfo.processIdentifier)"
        env["AICC_TOKEN_STDIN"] = "1"
        env["AICC_MAGPIE_URL"] = "http://127.0.0.1:\(AppSettings.shared.magpiePort)"
        env.removeValue(forKey: "CODEX_SANDBOX_NETWORK_DISABLED")
        env.removeValue(forKey: "CODEX_SESSION_ID")
        env.removeValue(forKey: "CODEX_THREAD_ID")
        env.removeValue(forKey: "CODEX_PERMISSION_PROFILE")
        if root.path.contains(".app/Contents/Resources/Server") {
            let dataDirectory = URL(fileURLWithPath: NSHomeDirectory())
                .appendingPathComponent("Library/Application Support/AICC-Dashboard/data", isDirectory: true)
            try? FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true)
            env["EINK_DATA_DIR"] = dataDirectory.path
        }
        process.environment = env

        // Set up log files
        let logsDir = AICCPaths.logsDirectory
        try? FileManager.default.createDirectory(at: logsDir, withIntermediateDirectories: true)

        let outLog = logsDir.appendingPathComponent("aicc-server.log")
        LogFileLimiter.trimIfNeeded(outLog)
        FileManager.default.createFile(atPath: outLog.path, contents: nil)
        if let handle = FileHandle(forWritingAtPath: outLog.path) {
            handle.seekToEndOfFile()
            process.standardOutput = handle
        }

        let errLog = logsDir.appendingPathComponent("aicc-server-error.log")
        LogFileLimiter.trimIfNeeded(errLog)
        FileManager.default.createFile(atPath: errLog.path, contents: nil)
        if let handle = FileHandle(forWritingAtPath: errLog.path) {
            handle.seekToEndOfFile()
            process.standardError = handle
        }

        do {
            guard !Task.isCancelled, AppSettings.shared.bridgeEnabled else { return false }
            let token = try BridgeCredential.loadOrCreate()
            let input = Pipe()
            process.standardInput = input
            stopReason = .none
            process.terminationHandler = { [weak self] _ in
                Task { @MainActor [weak self] in
                    self?.handleProcessTermination()
                }
            }
            try process.run()
            try input.fileHandleForWriting.write(contentsOf: Data((token + "\n").utf8))
            try input.fileHandleForWriting.close()
            serverProcess = process
            ownership = .managed
            isServerRunning = false
            let ready = await waitForServerAlive()
            if ready {
                isServerRunning = true
                healthState = .healthy
                return true
            }
            stopOwnedServer(reason: .recovery)
            healthState = .degraded
            return false
        } catch {
            print("Failed to start server: \(error)")
            healthState = .degraded
            return false
        }
    }

    func startMonitoring() {
        guard supervisorTask == nil else { return }
        supervisorTask = Task { @MainActor [weak self] in
            guard let firstSelf = self else { return }
            _ = await firstSelf.startServer()
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 60_000_000_000)
                if Task.isCancelled { break }
                guard let self else { break }
                await self.superviseOnce()
            }
        }
    }

    var dataDirectoryURL: URL? {
        guard let root = resolveServerRoot() else { return nil }
        if root.path.contains(".app/Contents/Resources/Server") {
            return URL(fileURLWithPath: NSHomeDirectory())
                .appendingPathComponent("Library/Application Support/AICC-Dashboard/data", isDirectory: true)
        }
        return root.appendingPathComponent("data", isDirectory: true)
    }

    func restartServer() async -> Bool {
        if ownership == .managed {
            stopOwnedServer(reason: .recovery)
            try? await Task.sleep(nanoseconds: 400_000_000)
        }
        restartFailures = 0
        nextRetryDelay = 1_000_000_000
        return await startServer()
    }

    func stopMonitoring() {
        supervisorTask?.cancel()
        supervisorTask = nil
    }

    /// The interpreter the bridge runs on.
    ///
    /// A release build uses the runtime inside the bundle and nothing else: the
    /// host's `/usr/bin/python3` is 3.9 with LibreSSL, so falling back silently
    /// would mean "LAN works on the developer's Mac and not on the user's".
    /// `AICC_DEV=1` is the only way to point it elsewhere.
    private func resolvePython() -> String? {
        let choice = PythonRuntime.locate(
            bundleURL: Bundle.main.bundleURL,
            environment: ProcessInfo.processInfo.environment,
            isExecutable: { FileManager.default.isExecutableFile(atPath: $0) }
        )
        if let reason = choice.message {
            runtimeError = reason
            NSLog("[AICC] %@", reason)
            return nil
        }
        if case .development(let path) = choice {
            NSLog("[AICC] development runtime in use: %@", path)
        }
        runtimeError = nil
        return choice.path
    }

    private struct LiveHealthResponse: Decodable {
        let ok: Bool?
        let status: String?
        let version: String?
        let build: String?
        let pid: Int?
        let ppid: Int?
        let serverInstanceID: String?
        let companion: Bool?
        /// 当前进程的监听范围（`loopback` / `lan`）。旧版本不返回，按不兼容处理：
        /// 否则升级之后那个绑在 `0.0.0.0` 的旧进程会被当成"兼容"继续复用。
        let bind: String?
        /// `lan_serving` 是"HTTPS 真的起来了"，`bind` 只是"打算开局域网"。
        /// 两者不一致时界面必须显示后者，不能让开关冒充监听状态。
        let lanServing: Bool?

        enum CodingKeys: String, CodingKey {
            case ok
            case status
            case version
            case build
            case pid
            case ppid
            case serverInstanceID = "server_instance_id"
            case companion
            case bind
            case lanServing = "lan_serving"
        }
    }

    private func checkServerIdentity() async -> (alive: Bool, compatible: Bool, trusted: Bool, isOrphan: Bool, pid: Int?) {
        guard let url = URL(string: "http://127.0.0.1:\(Self.productPort)/api/health/live") else {
            return (false, false, false, false, nil)
        }
        var request = URLRequest(url: url)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 1.0
        do {
            let (data, response) = try await URLSession.shared.data(for: request)
            guard (response as? HTTPURLResponse)?.statusCode == 200 else {
                return (false, false, false, false, nil)
            }
            guard let health = try? JSONDecoder().decode(LiveHealthResponse.self, from: data),
                  health.ok == true else {
                lanServing = false
                return (true, false, false, false, nil)
            }
            // 界面区分"用户打开了 LAN"与"HTTPS 真的起来了"：没有证书时
            // 桥接只开回环，这里必须是 false，不能让开关状态冒充监听状态。
            lanServing = health.lanServing ?? false
            let currentVersion = (Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String)?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let currentBuild = (Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String)?
                .trimmingCharacters(in: .whitespacesAndNewlines)

            let versionMatch = currentVersion == nil || currentVersion?.isEmpty == true || health.version == currentVersion
            let buildMatch = currentBuild != nil && !currentBuild!.isEmpty && health.build == currentBuild
            // 监听范围也必须一致：用户关掉"允许局域网设备连接"之后，
            // 旧进程还在 0.0.0.0 上服务的话，界面说的和实际就不是一回事。
            let expectedBind = AppSettings.shared.bridgeLANEnabled ? "lan" : "loopback"
            let bindMatch = health.bind == expectedBind
            let trusted = health.pid ?? 0 > 1
                && health.serverInstanceID.flatMap(UUID.init(uuidString:)) != nil
            let isOrphan = trusted && health.ppid == 1

            return (true, trusted && versionMatch && buildMatch && bindMatch && health.companion == true, trusted, isOrphan, health.pid)
        } catch {
            return (false, false, false, false, nil)
        }
    }

    private func reclaimPort(pid: Int?, trusted: Bool) async {
        guard trusted else { return }
        if let url = URL(string: "http://127.0.0.1:\(Self.productPort)/api/shutdown") {
            var request = URLRequest(url: url)
            request.httpMethod = "POST"
            request.timeoutInterval = 1.0
            _ = try? await URLSession.shared.data(for: request)
        }
        if let pid, pid > 1 {
            kill(pid_t(pid), SIGTERM)
            try? await Task.sleep(nanoseconds: 300_000_000)
            if kill(pid_t(pid), 0) == 0 {
                kill(pid_t(pid), SIGKILL)
            }
        }
        try? await Task.sleep(nanoseconds: 500_000_000)
    }

    private struct ReadyHealthResponse: Decodable {
        let ok: Bool?
        let status: String?
    }

    private func checkServerReadiness() async -> Bool {
        guard let url = URL(string: "http://127.0.0.1:\(Self.productPort)/api/health/ready") else {
            return false
        }
        var request = URLRequest(url: url)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 1.5
        do {
            let (data, response) = try await URLSession.shared.data(for: request)
            guard (response as? HTTPURLResponse)?.statusCode == 200 else {
                return false
            }
            guard let ready = try? JSONDecoder().decode(ReadyHealthResponse.self, from: data) else {
                return false
            }
            return ready.ok == true
        } catch {
            return false
        }
    }

    private func waitForServerAlive() async -> Bool {
        for _ in 0..<12 {
            let identity = await checkServerIdentity()
            if identity.alive && identity.compatible {
                let ready = await checkServerReadiness()
                if ready { return true }
            }
            try? await Task.sleep(nanoseconds: 500_000_000)
        }
        return false
    }

    private func superviseOnce() async {
        guard stopReason == .none || stopReason == .recovery else { return }
        let identity = await checkServerIdentity()
        if identity.alive && identity.compatible && !identity.isOrphan {
            let isReady = await checkServerReadiness()
            if isReady {
                isServerRunning = true
                healthState = .healthy
                restartFailures = 0
                nextRetryDelay = 1_000_000_000
                return
            } else {
                isServerRunning = false
                healthState = .degraded
                await reclaimPort(pid: identity.pid, trusted: identity.trusted)
                _ = await startServer()
                return
            }
        }

        if identity.alive && (!identity.compatible || identity.isOrphan) {
            isServerRunning = false
            healthState = .degraded
            await reclaimPort(pid: identity.pid, trusted: identity.trusted)
            _ = await startServer()
            return
        }

        isServerRunning = false
        healthState = .recovering
        if ownership == .managed {
            stopOwnedServer(reason: .recovery)
            try? await Task.sleep(nanoseconds: 400_000_000)
        }

        let now = Date()
        if now.timeIntervalSince(failureWindowStarted) > 300 {
            failureWindowStarted = now
            restartFailures = 0
            nextRetryDelay = 1_000_000_000
        }
        guard restartFailures < 5 else {
            healthState = .failed
            return
        }

        try? await Task.sleep(nanoseconds: nextRetryDelay)
        if await startServer() {
            restartFailures = 0
            nextRetryDelay = 1_000_000_000
        } else {
            restartFailures += 1
            nextRetryDelay = min(nextRetryDelay * 2, 30_000_000_000)
        }
    }

    private func handleProcessTermination() {
        serverProcess = nil
        isServerRunning = false
        ownership = .none
        healthState = stopReason == .appExit || stopReason == .user ? .stopped : .degraded
    }

    private func stopOwnedServer(reason: ServerStopReason, waitForExit: Bool = false) {
        guard ownership == .managed else { return }
        stopReason = reason
        let process = serverProcess
        process?.terminate()
        if waitForExit, let process, process.isRunning {
            let deadline = Date().addingTimeInterval(1.5)
            while process.isRunning && Date() < deadline {
                RunLoop.current.run(until: Date().addingTimeInterval(0.05))
            }
            if process.isRunning {
                kill(process.processIdentifier, SIGKILL)
            }
        }
        serverProcess = nil
        ownership = .none
        isServerRunning = false
        healthState = reason == .recovery ? .recovering : .stopped
    }

    func stopServer(reason: ServerStopReason = .user, waitForExit: Bool = false) {
        stopReason = reason
        stopOwnedServer(reason: reason, waitForExit: waitForExit)
        if ownership == .none {
            healthState = .stopped
        }
    }
}

// MARK: - App Delegate

@MainActor
class AICCAppDelegate: NSObject, NSApplicationDelegate {
    private let singleInstance = SingleInstanceService.shared
    private let serverManager = ServerManager.shared
    private var consoleCoordinator: ConsoleWindowCoordinator?
    private var consoleStore: ConsoleStore?
    private var settingsWindowCoordinator: SettingsWindowCoordinator?
    private var hasStartedShutdown = false
    private var hasPresentedMainWindow = false
    private var widgetRefreshTask: Task<Void, Never>?
    private var updateCheckTask: Task<Void, Never>?
    private var updateObserverCancellables = Set<AnyCancellable>()
    /// 菜单栏常驻图标。AICC 是 accessory 应用（没有 Dock 图标），
    /// 桥接开启时关掉窗口也不会退出 —— 没有这个入口，用户只能翻到
    /// 设置 → 高级与诊断 才能退出。
    private var statusItem: NSStatusItem?
    private var updateAvailableMenuItem: NSMenuItem?

    func applicationWillFinishLaunching(_ notification: Notification) {
        guard singleInstance.acquire() else {
            NSApp.terminate(nil)
            return
        }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        // A downloaded App runs from a read-only translocation copy while it
        // still carries quarantine, and a build with a new cdhash needs its
        // Widget extension re-registered.  Either one leaves the Desktop widget
        // as an empty black tile, so repair both before anything else.
        WidgetHealthService.repairIfNeeded()
        // Retire exact LaunchAgents from pre-DMG installs before ServerManager
        // checks port 8765 and starts the bundled server.
        LegacyLaunchAgentMigration.run()
        WidgetCenter.shared.reloadAllTimelines()

        settingsWindowCoordinator = SettingsWindowCoordinator(
            settings: AppSettings.shared,
            server: ServerManager.shared,
            loginAtLaunch: LaunchAtLoginService.shared,
            onClose: { [weak self] in self?.finishBackgroundRefresh() }
        )
        // 主控制台与设置共用同一个账号助手实例，避免一个 App 起两个 worker。
        let store = ConsoleStore(worker: .shared, settings: .shared)
        consoleStore = store
        consoleCoordinator = ConsoleWindowCoordinator(
            settings: AppSettings.shared,
            server: ServerManager.shared,
            loginAtLaunch: LaunchAtLoginService.shared,
            store: store,
            openSettings: { [weak self] in self?.presentSettings() },
            openUpdateSettings: { [weak self] in self?.presentSettings(category: .general) },
            onClose: { [weak self] in self?.finishBackgroundRefresh() }
        )

        installMenuBarItem()
        observeUpdateAvailability()

        if AppSettings.shared.bridgeEnabled { serverManager.startMonitoring() }
        // 自动旅行跑在账号助手进程里。上次退出前有账号开着自动旅行的话，
        // 这里就要把助手提前拉起来 —— 否则用户不打开控制台就没人调 tick。
        TravelAutomationService.shared.startIfNeeded(worker: .shared)
        let loginLaunch = NSAppleEventManager.shared().currentAppleEvent?
            .paramDescriptor(forKeyword: keyAEPropData)?.enumCodeValue == keyAELaunchedAsLogInItem
        // A footer tap on the widget launches us with a URL.  That is a page
        // turn, not someone opening settings: apply it and get out of the way
        // instead of throwing the window in their face every time.
        let urlLaunch = {
            guard let event = NSAppleEventManager.shared().currentAppleEvent else { return false }
            return event.eventClass == AEEventClass(kInternetEventClass)
                && event.eventID == AEEventID(kAEGetURL)
        }()
        if loginLaunch || urlLaunch {
            hasPresentedMainWindow = true
            if !AppSettings.shared.bridgeEnabled && loginLaunch { requestQuit() }
        } else if notification.userInfo?[NSApplication.launchIsDefaultUserInfoKey] as? Bool == false {
            hasPresentedMainWindow = true
        } else if !hasPresentedMainWindow {
            // 不再要求 NSApp.isActive：accessory 应用从 Finder / open 启动时
            // 这一刻还没被激活，以前那样判断会导致"点了图标什么都不出现"。
            presentConsole()
        }

        if !urlLaunch && !(!AppSettings.shared.bridgeEnabled && loginLaunch) {
            scheduleAutomaticUpdateCheck()
        }
    }

    func applicationDidBecomeActive(_ notification: Notification) {
        if consoleCoordinator != nil && !hasPresentedMainWindow { presentConsole() }
    }

    func application(_ application: NSApplication, open urls: [URL]) {
        // Footer taps on the usage board arrive as links, one URL per control:
        // aicc-widget://board/next, aicc-widget://board/previous, aicc-widget://refresh
        for url in urls where url.scheme == "aicc-widget" {
            if url.host == "board" {
                guard let step = AICCWidgetMetadata.boardStep(fromPath: url.path) else { continue }
                hasPresentedMainWindow = true
                WidgetPreferences.requestBoardStep(step)
                WidgetCenter.shared.reloadTimelines(ofKind: AICCWidgetMetadata.boardKind)
                finishBackgroundRefresh()
            } else {
                hasPresentedMainWindow = true
                refreshWidgets()
            }
        }
    }

    private func refreshWidgets() {
        guard widgetRefreshTask == nil else { return }
        widgetRefreshTask = Task {
            let snapshot = await WidgetStatusLoader.snapshot(force: true, port: AppSettings.shared.magpiePort)
            AppSettings.shared.publishWidgetSettings(providers: snapshot.providers)
            WidgetCenter.shared.reloadTimelines(ofKind: AICCWidgetMetadata.kind)
            WidgetCenter.shared.reloadTimelines(ofKind: AICCWidgetMetadata.boardKind)
            widgetRefreshTask = nil
            finishBackgroundRefresh()
        }
    }

    // MARK: - 菜单栏

    /// 菜单栏图标 + 三项菜单：打开主控制台 / 设置 / 退出。
    ///
    /// 只做导航与退出，不碰签到、授权、轮换或客户端切换。
    private func installMenuBarItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        if let button = item.button {
            let image = NSImage(
                systemSymbolName: "square.stack.3d.up.fill",
                accessibilityDescription: "AICC"
            )
            image?.isTemplate = true
            button.image = image
            button.toolTip = "AICC"
        }

        let menu = NSMenu()
        let updateBanner = NSMenuItem(
            title: "",
            action: #selector(menuOpenUpdateSettings),
            keyEquivalent: ""
        )
        updateBanner.target = self
        updateBanner.isHidden = true
        menu.addItem(updateBanner)
        updateAvailableMenuItem = updateBanner

        let console = NSMenuItem(
            title: "打开主控制台",
            action: #selector(menuOpenConsole),
            keyEquivalent: ""
        )
        console.target = self
        menu.addItem(console)

        let settings = NSMenuItem(
            title: "设置…",
            action: #selector(menuOpenSettings),
            keyEquivalent: ","
        )
        settings.target = self
        menu.addItem(settings)

        let checkUpdates = NSMenuItem(
            title: "检查更新…",
            action: #selector(menuCheckForUpdates),
            keyEquivalent: ""
        )
        checkUpdates.target = self
        menu.addItem(checkUpdates)

        menu.addItem(.separator())

        let quit = NSMenuItem(
            title: "退出 AICC",
            action: #selector(menuQuit),
            keyEquivalent: "q"
        )
        quit.target = self
        menu.addItem(quit)

        item.menu = menu
        statusItem = item
        syncUpdateMenuIndicator()
    }

    private func observeUpdateAvailability() {
        updateObserverCancellables.removeAll()
        UpdateService.shared.$state
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.syncUpdateMenuIndicator() }
            .store(in: &updateObserverCancellables)
        UpdateService.shared.$ignoredVersion
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.syncUpdateMenuIndicator() }
            .store(in: &updateObserverCancellables)
    }

    private func syncUpdateMenuIndicator() {
        if let update = UpdateService.shared.visibleUpdate {
            updateAvailableMenuItem?.title = "发现新版本 v\(update.version)…"
            updateAvailableMenuItem?.isHidden = false
            statusItem?.button?.toolTip = "AICC · 发现新版本 v\(update.version)"
        } else {
            updateAvailableMenuItem?.isHidden = true
            statusItem?.button?.toolTip = "AICC"
        }
    }

    private func scheduleAutomaticUpdateCheck() {
        updateCheckTask?.cancel()
        updateCheckTask = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 5_000_000_000)
            guard !Task.isCancelled, self != nil else { return }
            _ = await UpdateService.shared.checkForUpdatesIfDue(
                autoCheckEnabled: AppSettings.shared.autoCheckUpdates
            )
        }
    }

    @objc private func menuOpenConsole() {
        presentConsole()
    }

    @objc private func menuOpenSettings() {
        presentSettings()
    }

    @objc private func menuOpenUpdateSettings() {
        presentSettings(category: .general)
    }

    @objc private func menuCheckForUpdates() {
        presentSettings(category: .general, checkForUpdates: true)
    }

    @objc private func menuQuit() {
        requestQuit()
    }

    /// 主入口：启动、重新激活、点击 Dock 图标都走这里。
    private func presentConsole() {
        hasPresentedMainWindow = true
        consoleCoordinator?.present()
    }

    /// 右上角齿轮进入设置。设置不再作为默认首页。
    private func presentSettings(category: SettingsCategory? = nil, checkForUpdates: Bool = false) {
        settingsWindowCoordinator?.present(category: category, checkForUpdates: checkForUpdates)
    }

    func finishBackgroundRefresh() {
        guard !AppSettings.shared.bridgeEnabled,
              consoleCoordinator?.hasVisibleWindow != true,
              settingsWindowCoordinator?.hasVisibleWindow != true else { return }
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) { [weak self] in
            guard self?.consoleCoordinator?.hasVisibleWindow != true,
                  self?.settingsWindowCoordinator?.hasVisibleWindow != true,
                  !AppSettings.shared.bridgeEnabled else { return }
            self?.requestQuit()
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        !AppSettings.shared.bridgeEnabled
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        presentConsole()
        return true
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        shutDownForTermination()
        return .terminateNow
    }

    func applicationWillTerminate(_ notification: Notification) {
        shutDownForTermination()
    }

    private func requestQuit() {
        NSApp.terminate(nil)
    }

    private func shutDownForTermination() {
        guard !hasStartedShutdown else { return }
        hasStartedShutdown = true
        widgetRefreshTask?.cancel()
        widgetRefreshTask = nil
        updateCheckTask?.cancel()
        updateCheckTask = nil
        updateObserverCancellables.removeAll()
        updateAvailableMenuItem = nil
        TravelAutomationService.shared.stop()
        consoleCoordinator?.tearDown()
        consoleCoordinator = nil
        consoleStore = nil
        settingsWindowCoordinator?.tearDown()
        settingsWindowCoordinator = nil
        AccountWorkerManager.shared.stop()
        serverManager.stopMonitoring()
        serverManager.stopServer(reason: .appExit, waitForExit: true)
        statusItem = nil
    }
}

// MARK: - AppKit app shell

#if !AICC_VISUAL_TEST
@main
@MainActor
struct AICCApp {
    static func main() {
        let application = NSApplication.shared
        let delegate = AICCAppDelegate()
        application.delegate = delegate
        application.run()
    }
}
#endif
