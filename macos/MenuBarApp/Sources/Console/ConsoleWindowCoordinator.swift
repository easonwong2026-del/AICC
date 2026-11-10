import AppKit
import Combine
import SwiftUI

/// 主控制台窗口。整个 App 只允许存在一个实例：
/// 再次点击图标 / 从 Dock 激活时，把已有窗口提到前台，而不是再开一个。
@MainActor
final class ConsoleWindowCoordinator: NSObject, NSWindowDelegate {
    private let settings: AppSettings
    private let server: ServerManager
    private let loginAtLaunch: LaunchAtLoginService
    private let store: ConsoleStore
    private let openSettings: () -> Void
    private let onClose: () -> Void

    private var window: NSWindow?
    var hasVisibleWindow: Bool { window?.isVisible == true }
    // 关闭动画期间不能释放 SwiftUI 宿主视图（与 SettingsWindowCoordinator 同样的坑）。
    private var recentlyClosedWindows: [NSWindow] = []
    private var cancellables = Set<AnyCancellable>()

    init(
        settings: AppSettings,
        server: ServerManager,
        loginAtLaunch: LaunchAtLoginService,
        store: ConsoleStore,
        openSettings: @escaping () -> Void,
        onClose: @escaping () -> Void = {}
    ) {
        self.settings = settings
        self.server = server
        self.loginAtLaunch = loginAtLaunch
        self.store = store
        self.openSettings = openSettings
        self.onClose = onClose
        super.init()
        observeTheme()
    }

    func present() {
        NSApp.activate(ignoringOtherApps: true)

        if let window {
            if window.isMiniaturized { window.deminiaturize(nil) }
            window.makeKeyAndOrderFront(nil)
            window.orderFrontRegardless()
            return
        }

        let consoleWindow = makeWindow()
        consoleWindow.makeKeyAndOrderFront(nil)
        consoleWindow.orderFrontRegardless()
        window = consoleWindow
    }

    func tearDown() {
        guard let window else {
            cancellables.removeAll()
            return
        }
        window.delegate = nil
        window.close()
        self.window = nil
        cancellables.removeAll()
    }

    private func makeWindow() -> NSWindow {
        let hosting = NSHostingController(
            rootView: MainConsoleView(store: store, openSettings: openSettings)
                .environmentObject(settings)
                .environmentObject(server)
                .environmentObject(loginAtLaunch)
                .environment(\.locale, settings.locale)
                .preferredColorScheme(settings.preferredColorScheme)
                .id(settings.presentationIdentity)
        )
        let consoleWindow = NSWindow(contentViewController: hosting)
        consoleWindow.identifier = NSUserInterfaceItemIdentifier("com.aieink.dashboard.console")
        consoleWindow.styleMask = [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView]
        consoleWindow.title = "AICC"
        consoleWindow.titleVisibility = .hidden
        consoleWindow.titlebarAppearsTransparent = true
        consoleWindow.isMovableByWindowBackground = false
        consoleWindow.setContentSize(ConsoleTheme.windowSize)
        consoleWindow.minSize = ConsoleTheme.minWindowSize
        consoleWindow.isReleasedWhenClosed = false
        consoleWindow.animationBehavior = .none
        consoleWindow.delegate = self
        consoleWindow.center()
        return consoleWindow
    }

    /// 主题切换时重建标题栏外观，避免出现浅色窗口配深色标题栏。
    private func observeTheme() {
        settings.$themeMode
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in
                guard let self, let window = self.window else { return }
                DispatchQueue.main.async {
                    window.appearance = nil
                    window.invalidateShadow()
                }
            }
            .store(in: &cancellables)
    }

    func windowWillClose(_ notification: Notification) {
        guard let closingWindow = notification.object as? NSWindow, closingWindow === window else { return }
        closingWindow.delegate = nil
        window = nil
        recentlyClosedWindows.append(closingWindow)
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.0) { [weak self, weak closingWindow] in
            guard let closingWindow else { return }
            self?.recentlyClosedWindows.removeAll { $0 === closingWindow }
            self?.onClose()
        }
    }
}
