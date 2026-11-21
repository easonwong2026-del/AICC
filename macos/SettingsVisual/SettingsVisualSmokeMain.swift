import AppKit
import SwiftUI

/// 设置中心离屏渲染（Phase 2B）。
///
/// 真实 SwiftUI 视图 + 固定脱敏样本，**不**调用证书重置、撤销设备、凭据迁移、
/// LAN 修改或任何其他写操作：`SettingsModel.loadOnce()` 在双 flag 构建里直接
/// 返回，视图也不挂 `.task` 之外的读写。
@main
struct SettingsVisualSmokeMain {
    @MainActor
    static func main() throws {
        let outputDir = "/private/tmp/aicc-settings-qa"
        try FileManager.default.createDirectory(atPath: outputDir, withIntermediateDirectories: true)
        let application = NSApplication.shared
        application.setActivationPolicy(.prohibited)

        var count = 0
        let scenes = SettingsVisualScenes.basics + SettingsVisualScenes.states
        for scene in scenes {
            try renderScene(scene, to: outputDir)
            count += 1
        }

        // 独立详情与弹层：真实视图，不是网页模拟。
        try renderDetail(to: outputDir)
        count += 1
        try renderDetail(dark: true, to: outputDir)
        count += 1

        let empty = SettingsVisualState.healthy
        var emptyState = empty
        emptyState.migration = nil
        emptyState.verifyReport = nil
        try renderDetail(state: emptyState, suffix: "empty", to: outputDir)
        count += 1

        var withIssues = SettingsVisualState.healthy
        withIssues.migration = CredentialMigrationState(
            summary: CredentialMigrationSummary(
                migratable: 5, alreadyJSON: 1, magpie: 1, blocked: 2,
                rollbackable: 0, total: 9, needsRelink: 2
            ),
            items: SettingsVisualState.migrationNeeded.items,
            migratedRecords: 0,
            keyMigration: CredentialKeyMigration(failed: 1, pending: 0)
        )
        withIssues.verifyReport = CredentialVerifyReport(
            ok: 3, broken: 2, pendingKeychain: 5, orphanCount: 1,
            brokenFiles: 0, pendingCleanup: 0, needsRelink: 2
        )
        try renderDetail(state: withIssues, suffix: "issues", to: outputDir)
        count += 1

        // 展开态用高窗口，保证「迁移/回滚 + 盘点 + 逐条状态 + 最近结果」一次看全。
        try renderDetail(
            state: .healthy,
            suffix: "more",
            showMore: true,
            size: CGSize(width: 560, height: 980),
            to: outputDir
        )
        count += 1

        try renderSecurityDetail(dark: false, to: outputDir)
        count += 1
        try renderSecurityDetail(dark: true, to: outputDir)
        count += 1

        try renderPlaintextSheet(failure: nil, scheme: .light, to: outputDir)
        count += 1
        try renderPlaintextSheet(failure: nil, scheme: .dark, to: outputDir)
        count += 1
        try renderPlaintextSheet(
            failure: "设置没有保存成功，助手未运行。请稍后重试。",
            scheme: .light,
            to: outputDir
        )
        count += 1

        try renderQuotaSelection(to: outputDir)
        count += 1

        print("settings visual QA: \(count) images -> \(outputDir)")
    }

    // MARK: - 设置页

    @MainActor
    private static func renderScene(_ scene: SettingsVisualScenes.Scene, to outputDir: String) throws {
        let model = makeModel(scene.state, category: scene.category)
        scene.mutate(model)
        let scheme: ColorScheme = scene.scheme == .dark ? .dark : .light
        let view = SettingsView(model: model)
            .environmentObject(model.settings)
            .environmentObject(model.server)
            .environmentObject(model.loginAtLaunch)
            .environment(\.locale, model.settings.locale)
            .preferredColorScheme(scheme)
            .frame(width: scene.size.width, height: scene.size.height)
        try render(view, size: scene.size, scheme: scheme, to: "\(outputDir)/\(scene.name).png")
    }

    @MainActor
    static func makeModel(_ state: SettingsVisualState, category: SettingsCategory) -> SettingsModel {
        let worker = AccountWorkerManager.shared
        worker.applySettingsVisualState(state)

        let server = ServerManager.shared
        server.isServerRunning = state.serverRunning
        server.lanServing = state.lanServing
        server.runtimeError = state.runtimeError
        server.healthState = state.serverRunning ? .healthy : .stopped

        LaunchAtLoginService.shared.applySettingsVisualState(
            enabled: state.loginAtLaunchEnabled,
            error: state.loginAtLaunchError
        )

        let settings = AppSettings.shared
        if settings.bridgeEnabled != state.bridgeEnabled { settings.bridgeEnabled = state.bridgeEnabled }
        if settings.bridgeLANEnabled != state.bridgeLANEnabled { settings.bridgeLANEnabled = state.bridgeLANEnabled }
        if settings.magpiePort != state.magpiePort { settings.magpiePort = state.magpiePort }

        let model = SettingsModel(
            settings: settings,
            server: server,
            loginAtLaunch: .shared,
            worker: worker,
            navigation: SettingsNavigation(category: category)
        )
        model.connection = state.connection
        model.pairingCode = state.pairingCode
        model.securityNotice = state.securityNotice
        model.rotationTimezone = state.rotationTimezone
        model.rotationTimezones = state.rotationTimezones
        model.rotationRule = state.rotationRule
        model.rotationError = state.rotationError
        model.rotationLoaded = state.rotationLoaded
        model.tlsReset = state.tlsReset
        return model
    }

    // MARK: - 凭据详情

    @MainActor
    private static func renderDetail(dark: Bool = false, to outputDir: String) throws {
        try renderDetail(
            state: .healthy,
            suffix: dark ? "dark" : "light",
            scheme: dark ? .dark : .light,
            to: outputDir
        )
    }

    @MainActor
    private static func renderDetail(
        state: SettingsVisualState,
        suffix: String,
        scheme: ColorScheme = .light,
        showMore: Bool = false,
        size: CGSize = CGSize(width: 560, height: 520),
        to outputDir: String
    ) throws {
        AccountWorkerManager.shared.applySettingsVisualState(state)
        let view = CredentialMigrationDetailView(
            worker: .shared,
            onClose: {},
            height: size.height,
            autoRefresh: false,
            initialShowMore: showMore
        )
        .environment(\.colorScheme, scheme)
        try render(view.frame(width: size.width, height: size.height),
                   size: size, scheme: scheme,
                   to: "\(outputDir)/settings-credential-detail-\(Int(size.width))x\(Int(size.height))-\(suffix).png")
    }

    @MainActor
    private static func renderSecurityDetail(dark: Bool, to outputDir: String) throws {
        var state = SettingsVisualState.healthy
        state.bridgeEnabled = true
        state.bridgeLANEnabled = true
        state.serverRunning = true
        state.lanServing = true
        let model = makeModel(state, category: .connection)
        let scheme: ColorScheme = dark ? .dark : .light
        let view = BridgeSecurityDetailView(model: model, onClose: {})
            .environment(\.colorScheme, scheme)
        let size = CGSize(width: 520, height: 520)
        try render(view.frame(width: size.width, height: size.height),
                   size: size, scheme: scheme,
                   to: "\(outputDir)/settings-bridge-security-520x520-\(dark ? "dark" : "light").png")
    }

    // MARK: - 弹层

    @MainActor
    private static func renderPlaintextSheet(failure: String?, scheme: ColorScheme, to outputDir: String) throws {
        let view = PlaintextCredentialSheet(
            risk: "凭据目录是 0700、文件是 0600，但这只能挡住同机的其他用户，挡不住同一个 macOS 用户下运行的其他程序。",
            onConfirm: { false },
            onCancel: {},
            visualFailure: failure
        )
        let size = CGSize(width: 560, height: 560)
        let suffix = scheme == .dark ? "dark" : (failure == nil ? "light" : "failure")
        try render(view.frame(width: size.width, height: size.height),
                   size: size, scheme: scheme,
                   to: "\(outputDir)/settings-plaintext-sheet-560x560-\(suffix).png")
    }

    @MainActor
    private static func renderQuotaSelection(to outputDir: String) throws {
        // port 1 上没有服务：这张专门验收「读不到额度」的错误态与弹层高度。
        let view = QuotaSheetPreview()
        let size = CGSize(width: 480, height: 540)
        try render(view.frame(width: size.width, height: size.height),
                   size: size, scheme: .light,
                   to: "\(outputDir)/settings-quota-selection-error-480x540-light.png")
    }

    private struct QuotaSheetPreview: View {
        @State private var selection: [String] = ["codex:codex-1:5h", "claude:claude-1:5h"]

        var body: some View {
            QuotaSelectionView(port: 1, selection: $selection) { _ in }
        }
    }

    // MARK: - 渲染

    @MainActor
    static func render<V: View>(
        _ view: V,
        size: CGSize,
        scheme: ColorScheme,
        to path: String
    ) throws {
        let hosting = NSHostingView(rootView: view
            .environment(\.colorScheme, scheme)
            .preferredColorScheme(scheme))
        hosting.appearance = NSAppearance(named: scheme == .dark ? .darkAqua : .aqua)
        hosting.frame = NSRect(origin: .zero, size: size)
        hosting.layoutSubtreeIfNeeded()
        hosting.displayIfNeeded()
        RunLoop.current.run(until: Date().addingTimeInterval(0.45))
        hosting.layoutSubtreeIfNeeded()

        guard let bitmap = hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds) else {
            throw NSError(domain: "SettingsVisual", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "bitmap failed: \(path)"])
        }
        hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
        guard let data = bitmap.representation(using: .png, properties: [:]) else {
            throw NSError(domain: "SettingsVisual", code: 2,
                          userInfo: [NSLocalizedDescriptionKey: "png failed: \(path)"])
        }
        try data.write(to: URL(fileURLWithPath: path))
    }
}
