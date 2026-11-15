import AppKit
import SwiftUI

/// Qoder 的**备用**授权方式：Personal Access Token。
///
/// 为什么要有这条入口：设备授权依赖浏览器回调与 Qoder 的登录页，遇到网络策略
/// 或 SSO 限制就走不通；PAT 是用户在 Qoder 账户页自己生成的长期凭据，
/// 只要能联网就能用。
///
/// 三条界面纪律：
/// 1. 输入框是 `SecureField`，保存成功立刻清空，**界面上再也看不到它**；
/// 2. 保存成功只说"已保存并验证身份"，**不说"登录完成"**——能不能领 Credits
///    是另一件事，由额度页单独检测；
/// 3. 生成 PAT 需要离开 AICC，按钮直接把用户送到**对应站点**的账户页，
///    CN 与 International 的域名不同，不能共用一个链接。
struct QoderPatSheet: View {
    @ObservedObject var store: ConsoleStore
    let initialEdition: QoderEdition
    let onFinished: () -> Void

    @Environment(\.dismiss) private var dismiss

    @State private var edition: QoderEdition
    @State private var token = ""
    @State private var saving = false
    @State private var saved: QoderPatResult?
    @State private var failure: QoderPatFailure?

    init(store: ConsoleStore, initialEdition: QoderEdition, onFinished: @escaping () -> Void) {
        self.store = store
        self.initialEdition = initialEdition
        self.onFinished = onFinished
        _edition = State(initialValue: initialEdition)
    }

    private var canSave: Bool {
        !saving && saved == nil && token.trimmingCharacters(in: .whitespacesAndNewlines).count >= 20
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(ConsoleTheme.border)
            content
            Divider().overlay(ConsoleTheme.border)
            footer
        }
        .frame(width: 440)
        .background(ConsoleTheme.canvas)
    }

    private var header: some View {
        HStack(spacing: 10) {
            Image(systemName: "key.horizontal.fill")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(ConsoleTheme.accent)
                .frame(width: 28, height: 28)
                .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 8))
            VStack(alignment: .leading, spacing: 2) {
                Text("使用 Personal Access Token")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(ConsoleTheme.text)
                Text("备用方式 · 不需要安装客户端")
                    .font(.system(size: 11))
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
            Spacer(minLength: 0)
        }
        .padding(16)
    }

    private var content: some View {
        VStack(alignment: .leading, spacing: 14) {
            Picker("", selection: $edition) {
                ForEach(QoderEdition.allCases) { option in
                    Text(option.displayName).tag(option)
                }
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .tint(ConsoleTheme.accent)
            .disabled(saving || saved != nil)

            VStack(alignment: .leading, spacing: 6) {
                Text("1. 在 \(edition.displayName) 账户页生成一个 PAT")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(ConsoleTheme.text)
                Button {
                    if let url = edition.patPageURL { NSWorkspace.shared.open(url) }
                } label: {
                    Label("打开 \(edition.loginDomain)/account/integrations", systemImage: "safari")
                        .font(.system(size: 11))
                }
                .buttonStyle(.link)
                Text("生成的令牌只在那个页面显示一次，复制后立刻回到这里粘贴。")
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
            }

            VStack(alignment: .leading, spacing: 6) {
                Text("2. 粘贴到下面")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(ConsoleTheme.text)
                SecureField("粘贴 PAT", text: $token)
                    .textFieldStyle(.roundedBorder)
                    .font(.system(size: 12, design: .monospaced))
                    .disabled(saving || saved != nil)
                Text("令牌只写进系统钥匙串；AICC 的数据库、日志和截图里都不会出现它。")
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.tertiaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }

            resultBlock
        }
        .padding(.vertical, 18)
        .padding(.horizontal, 20)
    }

    @ViewBuilder
    private var resultBlock: some View {
        if let saved {
            VStack(alignment: .leading, spacing: 4) {
                Label("已保存，并已用这张 PAT 验证身份：\(saved.displayName)", systemImage: "checkmark.circle")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(ConsoleTheme.success)
                Text("身份验证通过不代表它有权领取 Credits。请在「额度」页对 \(saved.edition.displayName) 运行一次检测，看实际权限。")
                    .font(.system(size: 10))
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(ConsoleTheme.accentSoft, in: RoundedRectangle(cornerRadius: 8))
        } else if let failure {
            VStack(alignment: .leading, spacing: 4) {
                Label(failure.message, systemImage: "exclamationmark.triangle")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(ConsoleTheme.warning)
                    .fixedSize(horizontal: false, vertical: true)
                if !failure.hint.isEmpty {
                    Text(failure.hint)
                        .font(.system(size: 10))
                        .foregroundStyle(ConsoleTheme.secondaryText)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .padding(10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(ConsoleTheme.cardSubtle, in: RoundedRectangle(cornerRadius: 8))
        }
    }

    private var footer: some View {
        HStack(spacing: 10) {
            Spacer(minLength: 0)
            if saved == nil {
                Button("取消") { dismiss() }
                    .buttonStyle(ConsoleSecondaryButtonStyle())
                Button(saving ? "正在验证…" : "保存并验证") {
                    Task { await save() }
                }
                .buttonStyle(ConsolePrimaryButtonStyle())
                .disabled(!canSave)
            } else {
                Button("完成") {
                    onFinished()
                    dismiss()
                }
                .buttonStyle(ConsolePrimaryButtonStyle())
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
    }

    private func save() async {
        failure = nil
        saving = true
        defer { saving = false }
        let value = token.trimmingCharacters(in: .whitespacesAndNewlines)
        do {
            let outcome = try await store.worker.saveQoderPat(edition: edition, token: value)
            switch outcome {
            case .success(let result):
                // 令牌用完即弃：界面上不再保留，避免被截图或误复制。
                token = ""
                saved = result
                await store.refreshQoder(silent: true)
            case .failure(let error):
                failure = error
            }
        } catch {
            failure = .unknown((error as? LocalizedError)?.errorDescription ?? error.localizedDescription)
        }
    }
}
