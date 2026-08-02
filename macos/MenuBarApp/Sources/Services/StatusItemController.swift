import AppKit
import Combine
import SwiftUI

@MainActor
final class StatusItemController: NSObject, NSPopoverDelegate {
    private let api: APIService
    private let settings: AppSettings
    private let ocx: OpenCodexController
    private let openSettings: () -> Void

    private var statusItem: NSStatusItem?
    private var statusLabelView: NSHostingView<AnyView>?
    private var popover: NSPopover?
    private var hostingController: NSHostingController<AnyView>?
    private var secondaryClickRecognizer: NSClickGestureRecognizer?
    private var cancellables = Set<AnyCancellable>()
    private var isTearingDown = false

    private lazy var contextMenu: NSMenu = {
        let menu = NSMenu(title: "AICC")
        menu.addItem(menuItem("Open AICC Dashboard", action: #selector(openDashboardFromMenu)))
        menu.addItem(menuItem("Refresh All Now", action: #selector(refreshAllFromMenu)))
        menu.addItem(.separator())
        menu.addItem(menuItem("Settings…", action: #selector(openSettingsFromMenu)))
        menu.addItem(menuItem("Quit AICC", action: #selector(quitFromMenu)))
        return menu
    }()

    init(
        api: APIService,
        settings: AppSettings,
        ocx: OpenCodexController,
        openSettings: @escaping () -> Void
    ) {
        self.api = api
        self.settings = settings
        self.ocx = ocx
        self.openSettings = openSettings
        super.init()
        installStatusItem()
        observeChanges()
    }

    private func installStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.menu = nil
        statusItem = item

        guard let button = item.button else { return }
        button.title = ""
        button.image = nil
        button.isBordered = false
        button.target = self
        button.action = #selector(toggleDashboard)
        button.sendAction(on: [.leftMouseUp])

        let recognizer = NSClickGestureRecognizer(target: self, action: #selector(showContextMenu(_:)))
        recognizer.buttonMask = 1 << 1
        button.addGestureRecognizer(recognizer)
        secondaryClickRecognizer = recognizer

        let label = NSHostingView(rootView: statusLabel())
        label.translatesAutoresizingMaskIntoConstraints = false
        button.addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: button.leadingAnchor),
            label.trailingAnchor.constraint(equalTo: button.trailingAnchor),
            label.topAnchor.constraint(equalTo: button.topAnchor),
            label.bottomAnchor.constraint(equalTo: button.bottomAnchor)
        ])
        statusLabelView = label
        updateStatusItemLength()
    }

    private func observeChanges() {
        api.objectWillChange
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.updateStatusLabel() }
            .store(in: &cancellables)
        settings.objectWillChange
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.updateStatusLabel() }
            .store(in: &cancellables)
        // AppStorage updates UserDefaults but does not reliably forward an
        // ObservableObject change from AppSettings. Observe the defaults
        // notification so menu-bar visibility, theme, and locale snapshots
        // are refreshed immediately as well.
        NotificationCenter.default.publisher(for: UserDefaults.didChangeNotification)
            .receive(on: RunLoop.main)
            .sink { [weak self] _ in self?.updateStatusLabel() }
            .store(in: &cancellables)
    }

    private func statusLabel() -> AnyView {
        AnyView(
            MenuBarStatusLabel(
                status: api.status,
                showCodexStatus: settings.menuBarShowCodexStatus,
                showBalance: settings.menuBarShowCodexBalance
            )
            .environment(\.locale, settings.locale)
            .preferredColorScheme(settings.preferredColorScheme)
        )
    }

    private func updateStatusLabel() {
        guard !isTearingDown else { return }
        statusLabelView?.rootView = statusLabel()
        updateStatusItemLength()
    }

    private func updateStatusItemLength() {
        guard let statusItem, let statusLabelView else { return }
        statusLabelView.layoutSubtreeIfNeeded()
        let width = max(1, ceil(statusLabelView.fittingSize.width + 8))
        statusItem.length = width
    }

    private func makePopover() {
        let dashboard = DashboardView(openSettings: openSettings)
            .environmentObject(api)
            .environmentObject(ocx)
            .environmentObject(settings)
            .environment(\.locale, settings.locale)
            .preferredColorScheme(settings.preferredColorScheme)
        let controller = NSHostingController(rootView: AnyView(dashboard))
        let popover = NSPopover()
        popover.behavior = .transient
        popover.animates = true
        popover.delegate = self
        popover.contentViewController = controller
        popover.contentSize = NSSize(width: 350, height: max(1, controller.view.fittingSize.height))
        hostingController = controller
        self.popover = popover
    }

    @objc private func toggleDashboard() {
        guard !isTearingDown else { return }
        if popover?.isShown == true {
            popover?.performClose(nil)
        } else {
            guard let button = statusItem?.button else { return }
            if popover == nil { makePopover() }
            popover?.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        }
    }

    @objc private func showContextMenu(_ sender: NSClickGestureRecognizer) {
        guard sender.state == .ended, !isTearingDown, let button = statusItem?.button else { return }
        popover?.performClose(nil)
        contextMenu.popUp(positioning: nil, at: NSPoint(x: button.bounds.midX, y: button.bounds.minY), in: button)
    }

    @objc private func openDashboardFromMenu() {
        DispatchQueue.main.async { [weak self] in self?.toggleDashboardIfNeeded() }
    }

    private func toggleDashboardIfNeeded() {
        guard !isTearingDown else { return }
        if popover == nil { makePopover() }
        guard let button = statusItem?.button, let popover else { return }
        if popover.isShown {
            popover.contentViewController?.view.window?.makeKeyAndOrderFront(nil)
        } else {
            popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
        }
    }

    @objc private func refreshAllFromMenu() {
        Task { @MainActor [weak self] in
            guard let self, !self.isTearingDown else { return }
            await api.fetchStatus(force: true)
        }
    }

    @objc private func openSettingsFromMenu() {
        DispatchQueue.main.async { [weak self] in self?.openSettings() }
    }

    @objc private func quitFromMenu() {
        NSApp.terminate(nil)
    }

    private func menuItem(_ key: String, action: Selector) -> NSMenuItem {
        let item = NSMenuItem(title: settings.localized(key), action: action, keyEquivalent: "")
        item.target = self
        return item
    }

    func popoverWillShow(_ notification: Notification) {
        ocx.panelDidAppear()
    }

    func popoverDidClose(_ notification: Notification) {
        ocx.panelDidDisappear()
    }

    func tearDown() {
        guard !isTearingDown else { return }
        isTearingDown = true
        cancellables.removeAll()
        if let button = statusItem?.button, let recognizer = secondaryClickRecognizer {
            button.removeGestureRecognizer(recognizer)
        }
        popover?.performClose(nil)
        popover?.delegate = nil
        popover?.contentViewController = nil
        popover = nil
        hostingController = nil
        statusLabelView?.removeFromSuperview()
        statusLabelView = nil
        ocx.panelDidDisappear()
        if let statusItem {
            NSStatusBar.system.removeStatusItem(statusItem)
        }
        self.statusItem = nil
        contextMenu.items.forEach { $0.target = nil }
    }
}
