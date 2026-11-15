import AppKit
import CoreImage
import CoreImage.CIFilterBuiltins
import SwiftUI

/// 添加 WorkBuddy 账号：**浏览器登录为主**，扫码为次，两者共用同一个会话。
///
/// 流程完全走官方 `auth/state` → `auth/token`，不读也不改 WorkBuddy 客户端
/// 自己的登录态；令牌由助手直接写进钥匙串，这个界面拿不到 token。
///
/// 国内版与国际版是**两套 OAuth 与两套域名**：页面上必须选清楚，
/// 登录会话也不会在完成后被"看起来像"的 domain 改档。
struct AddWorkbuddyAccountSheet: View {
    @ObservedObject var store: ConsoleStore
    let onFinished: () -> Void

    @Environment(\.dismiss) private var dismiss

    @State private var start: WorkbuddyLoginStart?
    @State private var phase: WorkbuddyLoginPhase = .pending
    @State private var failure: String?
    @State private var starting = false
    @State private var deadline: Date?
    @State private var now = Date()
    @State private var pollTask: Task<Void, Never>?
    @State private var variant: WorkbuddyVariant = .cn

    /// 嵌在统一添加面板里时不再画自己的标题（外层已有面包屑），
    /// 单独作为 sheet 使用时保留标题。
    private let showsHeader: Bool

    init(
        store: ConsoleStore,
        initialVariant: WorkbuddyVariant = .cn,
        showsHeader: Bool = false,
        onFinished: @escaping () -> Void
    ) {
        self.store = store
        self.onFinished = onFinished
        self.showsHeader = showsHeader
        _variant = State(initialValue: initialVariant)
    }

    private var isSuccess: Bool {
        if case .success = phase { return true }
        return false
    }

    private var isTerminal: Bool { phase.isTerminal }

    var body: some View {
        VStack(spacing: 0) {
            if showsHeader {
                header
                Divider().overlay(ConsoleTheme.border)
            }
            // 内容区独立滚动：最小窗口下二维码与状态说明再多也不会把底部按钮挤出去。
            ScrollView { content }
                .frame(maxHeight: 420)
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
            Image(systemName: "person.badge.key.fill")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(ConsoleTheme.accent)
                .frame(width: 28, height: 28)
                .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 8))
            VStack(alignment: .leading, spacing: 2) {
                Text("添加 \(variant.title) 账号").font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(ConsoleTheme.text)
                Text(variant.subtitle)
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
            Spacer(minLength: 0)
        }
        .padding(16)
    }

    @ViewBuilder
    private var content: some View {
        VStack(spacing: 14) {
            variantPicker
            browserBlock
            qrBlock
            statusLine
        }
        .padding(.vertical, 18)
        .padding(.horizontal, 20)
    }

    /// 选站点。切换即重开一个会话：state 与后续 token 都是按站点签发的。
    private var variantPicker: some View {
        Picker("", selection: $variant) {
            ForEach(WorkbuddyVariant.allCases) { option in
                Text(option.title).tag(option)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .tint(ConsoleTheme.accent)
        .disabled(starting || isSuccess)
        .onChange(of: variant) { _, _ in
            guard !starting, !isSuccess else { return }
            Task { await begin() }
        }
    }

    /// 主入口：在系统默认浏览器里完成授权。不要求回调到 AICC，也不装扩展。
    private var browserBlock: some View {
        VStack(spacing: 8) {
            Button {
                openInBrowser()
            } label: {
                Label("在浏览器中登录", systemImage: "safari")
                    .frame(maxWidth: .infinity)
            }
            .buttonStyle(ConsolePrimaryButtonStyle())
            .disabled(start == nil || starting || isTerminal)

            Text("浏览器里登录完成后这里会自动刷新；也可以用下面的二维码在手机上扫。")
                .font(.system(size: 11))
                .foregroundStyle(ConsoleTheme.tertiaryText)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func openInBrowser() {
        // 地址来自服务端：只接受本站点的 https 登录页，其余 scheme / 域名一律不打开。
        guard let start, let url = variant.trustedLoginURL(start.verificationUri) else {
            failure = "登录地址不可信，已拒绝打开"
            phase = .failed(kind: "untrusted_url", message: failure ?? "登录地址不可信")
            return
        }
        NSWorkspace.shared.open(url)
    }

    private var qrBlock: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 12)
                .fill(Color.white)
                .overlay(
                    RoundedRectangle(cornerRadius: 12).strokeBorder(ConsoleTheme.border, lineWidth: 1)
                )
            // 二维码内容同样按可信域名过滤：扫出去的就是这个字符串，不能是任意链接。
            if let start,
               let url = variant.trustedLoginURL(start.verificationUri),
               let image = Self.qrImage(from: url.absoluteString) {
                Image(nsImage: image)
                    .interpolation(.none)
                    .resizable()
                    .frame(width: 208, height: 208)
                    .blur(radius: dimmed ? 5 : 0)
                    .opacity(dimmed ? 0.35 : 1)
            } else if starting {
                ProgressView().controlSize(.small)
            } else {
                Image(systemName: "qrcode")
                    .font(.system(size: 44, weight: .light))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
            }
            if dimmed {
                overlayMessage
            }
        }
        .frame(width: 240, height: 240)
    }

    private var dimmed: Bool {
        switch phase {
        case .pending: return false
        default: return true
        }
    }

    @ViewBuilder
    private var overlayMessage: some View {
        switch phase {
        case .success:
            label("checkmark.circle.fill", "已添加", ConsoleTheme.success)
        case .expired:
            label("clock.badge.exclamationmark", "二维码已过期", ConsoleTheme.warning)
        case .cancelled:
            label("xmark.circle", "已取消", ConsoleTheme.secondaryText)
        case .failed:
            label("exclamationmark.triangle", "登录失败", ConsoleTheme.danger)
        default:
            label("hourglass", "处理中", ConsoleTheme.secondaryText)
        }
    }

    private func label(_ symbol: String, _ text: String, _ tint: Color) -> some View {
        VStack(spacing: 6) {
            Image(systemName: symbol).font(.system(size: 22))
            Text(text).font(.system(size: 12, weight: .medium))
        }
        .foregroundStyle(tint)
        .padding(14)
        .background(ConsoleTheme.card.opacity(0.92), in: RoundedRectangle(cornerRadius: 10))
    }

    @ViewBuilder
    private var statusLine: some View {
        if let failure {
            Text(failure)
                .font(.system(size: 11))
                .foregroundStyle(ConsoleTheme.danger)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
        } else {
            switch phase {
            case .pending:
                VStack(spacing: 4) {
                    Text("等待\(variant.title)授权确认…")
                        .font(.system(size: 12))
                        .foregroundStyle(ConsoleTheme.text)
                    if let deadline {
                        Text("二维码 \(remainingText(until: deadline)) 后过期")
                            .font(.system(size: 11))
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                    }
                }
            case .success(let reused):
                Text(reused ? "账号已存在，登录状态已更新" : "登录成功，账号已保存")
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(ConsoleTheme.success)
            case .expired:
                Text("二维码已过期，点「重新获取」再试一次")
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
            case .cancelled:
                Text("已取消，没有写入任何账号")
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
            case .finished:
                Text("登录已结束，账号列表已刷新")
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
            case .failed(_, let message):
                Text(message)
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.danger)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var footer: some View {
        HStack(spacing: 8) {
            Text("凭据保存在本机钥匙串，不会写入数据库或日志")
                .font(.system(size: 11))
                .foregroundStyle(ConsoleTheme.tertiaryText)
            Spacer(minLength: 8)
            if !isSuccess {
                Button("取消") {
                    pollTask?.cancel()
                    if let start { Task { await store.worker.cancelWorkbuddyLogin(start.loginId) } }
                    dismiss()
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
            }
            if canRetry {
                Button("重新获取") { Task { await begin() } }
                    .buttonStyle(ConsolePrimaryButtonStyle())
            } else if isSuccess {
                Button("完成") {
                    onFinished()
                    dismiss()
                }
                .buttonStyle(ConsolePrimaryButtonStyle())
            }
        }
        .padding(14)
    }

    private var canRetry: Bool {
        switch phase {
        case .expired, .cancelled, .failed, .finished: return !starting
        default: return false
        }
    }

    // MARK: - 行为

    private func begin() async {
        guard !starting else { return }
        starting = true
        failure = nil
        phase = .pending
        pollTask?.cancel()
        defer { starting = false }

        if let previous = start {
            await store.worker.cancelWorkbuddyLogin(previous.loginId)
        }

        do {
            let session = try await store.worker.startWorkbuddyLogin(variant: variant)
            start = session
            deadline = Date().addingTimeInterval(TimeInterval(session.expiresIn))
            startPolling(session.loginId)
        } catch {
            start = nil
            failure = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            phase = .failed(kind: "start", message: failure ?? "无法开始登录")
        }
    }

    private func startPolling(_ loginId: String) {
        pollTask?.cancel()
        pollTask = Task { @MainActor in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 1_500_000_000)
                if Task.isCancelled { return }
                do {
                    let next = try await store.worker.pollWorkbuddyLogin(loginId)
                    phase = next
                    switch next {
                    case .pending:
                        continue
                    case .success:
                        await store.refreshAccounts()
                        onFinished()
                        return
                    default:
                        return
                    }
                } catch {
                    // 单次轮询失败（超时/网络）不结束会话，继续等，避免偶发抖动毁掉登录。
                    continue
                }
            }
        }
    }

    private func remainingText(until deadline: Date) -> String {
        let seconds = max(0, Int(deadline.timeIntervalSince(now)))
        if seconds >= 60 { return "\(seconds / 60) 分 \(seconds % 60) 秒" }
        return "\(seconds) 秒"
    }

    /// 官方登录页地址直接编码成二维码，不额外请求图片。
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
