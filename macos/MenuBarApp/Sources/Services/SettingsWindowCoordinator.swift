import AppKit
import Combine
import SwiftUI

struct SettingsRootView: View {
    @ObservedObject var settings: AppSettings
    let api: APIService
    let ocx: OpenCodexController
    let server: ServerManager
    let loginAtLaunch: LaunchAtLoginService

    var body: some View {
        SettingsView()
            .environmentObject(api)
            .environmentObject(ocx)
            .environmentObject(settings)
            .environmentObject(server)
            .environmentObject(loginAtLaunch)
            .environment(\.locale, settings.locale)
            .preferredColorScheme(settings.preferredColorScheme)
            .id(settings.presentationIdentity)
    }
}

/// Owns the one AppKit settings window used by both settings entry points.
/// The SwiftUI Settings scene is intentionally not used for presentation:
/// accessory apps cannot reliably materialize that scene from a menu action.
@MainActor
final class SettingsWindowCoordinator: NSObject, NSWindowDelegate {
    private let api: APIService
    private let ocx: OpenCodexController
    private let settings: AppSettings
    private let server: ServerManager
    private let loginAtLaunch: LaunchAtLoginService

    private var window: NSWindow?
    private var cancellables = Set<AnyCancellable>()
    private var lifecycle = SettingsWindowLifecycle()

    init(
        api: APIService,
        ocx: OpenCodexController,
        settings: AppSettings,
        server: ServerManager,
        loginAtLaunch: LaunchAtLoginService
    ) {
        self.api = api
        self.ocx = ocx
        self.settings = settings
        self.server = server
        self.loginAtLaunch = loginAtLaunch
        super.init()
        observeSettings()
    }

    var isPresented: Bool {
        lifecycle.state == .presented && window != nil
    }

    func present() {
        NSApp.activate(ignoringOtherApps: true)

        if let settingsWindow = window {
            if settingsWindow.isMiniaturized {
                settingsWindow.deminiaturize(nil)
            }
            settingsWindow.makeKeyAndOrderFront(nil)
            settingsWindow.orderFrontRegardless()
            return
        }

        guard lifecycle.beginPresentation() else { return }
        let settingsWindow = makeWindow()
        if settingsWindow.isMiniaturized {
            settingsWindow.deminiaturize(nil)
        }
        settingsWindow.makeKeyAndOrderFront(nil)
        settingsWindow.orderFrontRegardless()
        updateWindowTitle()
    }

    func tearDown() {
        guard let settingsWindow = window else {
            cancellables.removeAll()
            lifecycle.close()
            return
        }
        settingsWindow.delegate = nil
        settingsWindow.contentViewController = nil
        settingsWindow.close()
        window = nil
        lifecycle.close()
        cancellables.removeAll()
    }

    private func makeWindow() -> NSWindow {
        let rootView = SettingsRootView(
            settings: settings,
            api: api,
            ocx: ocx,
            server: server,
            loginAtLaunch: loginAtLaunch
        )
        let hostingController = NSHostingController(rootView: rootView)
        let settingsWindow = NSWindow(contentViewController: hostingController)
        settingsWindow.identifier = NSUserInterfaceItemIdentifier("com.aieink.dashboard.settings")
        settingsWindow.styleMask = [.titled, .closable, .miniaturizable, .resizable]
        settingsWindow.setContentSize(NSSize(width: 640, height: 480))
        settingsWindow.minSize = NSSize(width: 560, height: 420)
        settingsWindow.isReleasedWhenClosed = true
        settingsWindow.delegate = self
        settingsWindow.center()
        window = settingsWindow
        updateWindowTitle()
        return settingsWindow
    }

    private func observeSettings() {
        settings.$languageCode
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in
                DispatchQueue.main.async { [weak self] in self?.updateWindowTitle() }
            }
            .store(in: &cancellables)
    }

    private func updateWindowTitle() {
        window?.title = settings.localized("Settings")
    }

    func windowWillClose(_ notification: Notification) {
        guard let closingWindow = notification.object as? NSWindow, closingWindow === window else { return }
        closingWindow.delegate = nil
        closingWindow.contentViewController = nil
        window = nil
        lifecycle.close()
    }
}
