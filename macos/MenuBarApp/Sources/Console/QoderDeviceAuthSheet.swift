import AppKit
import CoreImage
import CoreImage.CIFilterBuiltins
import SwiftUI

/// 添加 Qoder 账号：**设备授权**，不要求装 Qoder 客户端。
///
/// 流程：`qoder.auth.start` 拿到该版本的 `/device/selectAccounts` 登录页 →
/// 用户在自己浏览器里登录并授权 → 界面每 2 秒轮询一次 `qoder.auth.poll`。
/// 令牌由助手直接写进钥匙串，这个界面拿不到 token。
///
/// 这个面板是**独立组件**：不复用也不改动 WorkBuddy 的登录面板，
/// 两个站点的登录流程互不影响（`QoderLoginStart` / `QoderLoginPhase` 各自一套模型）。
struct QoderDeviceAuthSheet: View {
    @ObservedObject var store: ConsoleStore
    let onFinished: () -> Void

    @Environment(\.dismiss) private var dismiss

    @State private var edition: QoderEdition
    @State private var start: QoderLoginStart?
    @State private var phase: QoderLoginPhase = .pending
    @State private var failure: String?
    @State private var starting = false
    @State private var deadline: Date?
    @State private var now = Date()
    @State private var pollTask: Task<Void, Never>?

    init(store: ConsoleStore, initialEdition: QoderEdition, onFinished: @escaping () -> Void) {
        self.store = store
        self.onFinished = onFinished
        _edition = State(initialValue: initialEdition)
    }

    private var isSuccess: Bool {
        if case .success = phase { return true }
        return false
    }

    private var isTerminal: Bool { phase.isTerminal }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(ConsoleTheme.border)
            content
            Divider().overlay(ConsoleTheme.border)
            footer
        }
        .frame(width: 420)
        .background(ConsoleTheme.canvas)
        .task { await begin() }
        .onDisappear { pollTask?.cancel() }
        .onReceive(Timer.publish(every: 1, on: .main, in: .common).autoconnect()) { value in
            now = value
        }
    }

    private var header: some View {
        HStack(spacing: 10) {
            Image(systemName: "rectangle.and.hand.point.up.left.filled")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(ConsoleTheme.accent)
                .frame(width: 28, height: 28)
                .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 8))
            VStack(alignment: .leading, spacing: 2) {
                Text("添加 \(edition.displayName) 账号")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(ConsoleTheme.text)
                Text("浏览器设备授权 · 不需要安装客户端")
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
            Spacer(minLength: 0)
        }
        .padding(16)
    }

    private var content: some View {
        VStack(spacing: 14) {
            Picker("", selection: $edition) {
                ForEach(QoderEdition.allCases) { option in
                    Text(option.displayName).tag(option)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .tint(ConsoleTheme.accent)
            .disabled(starting || isSuccess)
            .onChange(of: edition) { _, _ in
                guard !starting, !isSuccess else { return }
                Task { await begin() }
            }

            Button {
                openInBrowser()
            } label: {
                Label("在浏览器中打开授权页", systemImage: "safari")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(ConsolePrimaryButtonStyle())
            .disabled(start == nil || starting || isTerminal)

            Text("浏览器里选好账号并确认授权后，这里会自动完成；也会显示二维码，方便换到手机上扫。")
                .font(.system(size: 10))
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)

            qrBlock
            statusLine
        }
        .padding(.vertical, 18)
        .padding(.horizontal, 20)
    }

    private var qrBlock: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 12)
                .fill(Color.white)
                .overlay(
                    RoundedRectangle(cornerRadius: 12)
                        .strokeBorder(ConsoleTheme.border, lineWidth: 1)
                )
            // 二维码内容必须与打开的是同一个可信地址。
            if let start, let url = start.trustedURL, let image = Self.qrImage(from: url.absoluteString) {
                Image(nsImage: image)
                    .interpolation(.none)
                    .resizable()
                    .frame(width: 180, height: 180)
                    .blur(radius: isTerminal ? 5 : 0)
                    .opacity(isTerminal ? 0.35 : 1)
            } else if starting {
                ProgressView().controlSize(.small)
            } else {
                Image(systemName: "qrcode")
                    .font(.system(size: 40, weight: .light))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
            }
            if isTerminal {
                Text(terminalLabel)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 5)
                    .background(ConsoleTheme.canvas.opacity(0.92), in: Capsule())
            }
        }
        .frame(width: 208, height: 208)
    }

    private var terminalLabel: String {
        switch phase {
        case .success: return "已添加"
        case .expired: return "已过期"
        case .cancelled: return "已取消"
        case .finished: return "已完成"
        default: return "未完成"
        }
    }

    private var statusLine: some View {
        VStack(spacing: 4) {
            switch phase {
            case .pending:
                Text(starting
                    ? "正在获取授权页…"
                    : "等待浏览器授权 · 剩余 \(remainingText())")
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
            case .success(let name):
                Text("已添加 \(name)")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(ConsoleTheme.success)
            case .cancelled:
                Text("已取消，可以重新发起")
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
            case .expired:
                Text("授权页已过期，请重新发起")
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
            case .finished:
                Text("这次授权已结束")
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
            case .failed(_, let message):
                Text(message)
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .multilineTextAlignment(.center)
            }
            if let failure {
                Text(failure)
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .multilineTextAlignment(.center)
            }
        }
        .frame(maxWidth: .infinity)
    }

    private var footer: some View {
        HStack(spacing: 10) {
            if isTerminal && !isSuccess {
                Button("重新发起") {
                    Task { await begin() }
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
            }
            Spacer(minLength: 0)
            if isSuccess {
                Button("完成") { dismiss() }
                    .buttonStyle(ConsolePrimaryButtonStyle())
            } else {
                Button("关闭") {
                    if let start, !isTerminal {
                        Task { await store.worker.cancelQoderLogin(start.loginId) }
                    }
                    dismiss()
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
    }

    private func remainingText() -> String {
        guard let deadline else { return "—" }
        let seconds = max(0, Int(deadline.timeIntervalSince(now)))
        if seconds >= 60 { return "\(seconds / 60) 分 \(seconds % 60) 秒" }
        return "\(seconds) 秒"
    }

    private func openInBrowser() {
        guard let start, let url = start.trustedURL else {
            failure = "授权地址不是 \(edition.loginDomain) 的 https 页面，已拒绝打开"
            return
        }
        NSWorkspace.shared.open(url)
    }

    private func begin() async {
        let previous = start
        failure = nil
        phase = .pending
        start = nil
        starting = true
        pollTask?.cancel()
        defer { starting = false }

        if let previous {
            await store.worker.cancelQoderLogin(previous.loginId)
        }

        do {
            let session = try await store.worker.startQoderLogin(edition: edition)
            start = session
            deadline = Date().addingTimeInterval(TimeInterval(session.expiresIn))
            startPolling(session.loginId)
        } catch {
            start = nil
            failure = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            phase = .failed(kind: "start", message: "无法发起授权")
        }
    }

    private func startPolling(_ loginId: String) {
        pollTask?.cancel()
        pollTask = Task { @MainActor in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 2_000_000_000)
                if Task.isCancelled { return }
                do {
                    let next = try await store.worker.pollQoderLogin(loginId)
                    phase = next
                    switch next {
                    case .pending:
                        continue
                    case .success:
                        await store.refreshQoder(silent: true)
                        onFinished()
                        return
                    default:
                        return
                    }
                } catch {
                    // 单次轮询失败不结束会话：继续等，避免偶发抖动毁掉授权。
                    continue
                }
            }
        }
    }

    /// 登录页直接编码成二维码，不额外请求图片。
    static func qrImage(from string: String) -> NSImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(string.utf8)
        filter.correctionLevel = "M"
        guard let output = filter.outputImage else { return nil }
        let scaled = output.transformed(by: CGAffineTransform(scaleX: 8, y: 8))
        let rep = NSCIImageRep(ciImage: scaled)
        let image = NSImage(size: rep.size)
        image.addRepresentation(rep)
        return image
    }
}
