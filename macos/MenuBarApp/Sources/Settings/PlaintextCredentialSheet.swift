import SwiftUI

/// 首次启用明文 JSON 的风险说明与确认（CRED-3-UI）。
///
/// 只在用户**主动**点「启用本地 JSON」时出现。点「仍使用钥匙串」什么都不写 ——
/// 助手的默认行为本来就是钥匙串，所以拒绝是零成本的。
///
/// 文案与语义沿用 CRED-3/4 的交付，只换成 UI V5 的排版与配色。
struct PlaintextCredentialSheet: View {
    /// 助手给出的风险说明（`health.credentials.risk`）。为空时用本地文案兜底。
    let risk: String
    /// 返回是否保存成功；失败时留在本页并显示原因。
    let onConfirm: () async -> Bool
    let onCancel: () -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var busy = false
    @State private var failure: String?

    /// 离屏渲染：直接呈现"保存失败"的样子。生产构建里没有这个参数。
    #if AICC_VISUAL_TEST && AICC_SETTINGS_VISUAL
    var visualFailure: String?
    #endif

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    Text("之后新建的 WorkBuddy 与 Qoder 自有授权（设备授权、PAT）会保存为本机 JSON 文件，不再写入钥匙串。这样后台任务不会因为 App 重新签名而反复弹出授权框。")
                        .font(ConsoleType.body)
                        .foregroundStyle(ConsoleTheme.text)
                        .fixedSize(horizontal: false, vertical: true)

                    point(
                        icon: "doc.plaintext",
                        title: "凭据是明文保存的",
                        detail: risk.isEmpty
                            ? "凭据目录是 0700、文件是 0600，但这只能挡住同机的其他用户，挡不住同一个 macOS 用户下运行的其他程序。"
                            : risk,
                        tone: .warning
                    )
                    point(
                        icon: "externaldrive.badge.exclamationmark",
                        title: "建议开启 FileVault",
                        detail: "不要把凭据目录放进云同步（iCloud、Dropbox 等），也不要把明文 JSON 放进未加密的备份或诊断包。"
                    )
                    point(
                        icon: "clock.arrow.circlepath",
                        title: "之后新建的授权默认写 JSON",
                        detail: "新的自有授权直接落成本机文件，也允许把旧账号迁过来（在「设置 → 授权与存储 → 管理凭据」里显式操作）。两件事共用这一个开关。"
                    )
                    point(
                        icon: "arrow.uturn.backward",
                        title: "随时可以改回去",
                        detail: "在「设置 → 授权与存储」里关闭后，之后新建的授权重新使用钥匙串；已经写成 JSON 的授权不会因此失效，仍可在「管理凭据」里回滚。"
                    )
                    point(
                        icon: "key",
                        title: "钥匙串仍会用到",
                        detail: "Qoder 客户端自身的登录凭据、导入时读取的第三方条目不受影响；开关打开之前保存的 AICC 授权也仍在钥匙串里。"
                    )

                }
                .padding(.horizontal, ConsoleTheme.Sheet.padding)
                .padding(.top, 16)
                .padding(.bottom, 18)
            }
            .frame(maxHeight: .infinity)

            Divider()

            if let failure {
                ConsoleInlineNotice(kind: .error, message: failure)
                    .padding(.horizontal, ConsoleTheme.Sheet.padding)
                    .padding(.top, 12)
            }

            HStack(spacing: 8) {
                Button("仍使用钥匙串") {
                    onCancel()
                    dismiss()
                }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .disabled(busy)

                Spacer()

                if busy {
                    ProgressView().controlSize(.small)
                }

                Button("我已了解，启用") {
                    Task {
                        busy = true
                        defer { busy = false }
                        if await onConfirm() {
                            dismiss()
                        } else {
                            failure = "设置没有保存成功，助手可能未运行。请稍后重试。"
                        }
                    }
                }
                .buttonStyle(ConsolePrimaryButtonStyle())
                .keyboardShortcut(.defaultAction)
                .disabled(busy)
            }
            .padding(.horizontal, ConsoleTheme.Sheet.padding)
            .padding(.vertical, 14)
        }
        .background(ConsoleTheme.canvas)
        .frame(width: ConsoleTheme.Sheet.mediumWidth, height: 560)
        #if AICC_VISUAL_TEST && AICC_SETTINGS_VISUAL
        .onAppear {
            if let visualFailure, failure == nil { failure = visualFailure }
        }
        #endif
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 2) {
                Text("启用本地 JSON 存储")
                    .font(ConsoleType.detailTitle)
                    .foregroundStyle(ConsoleTheme.text)
                Text("明文保存，只有你能决定是否开启")
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
            }
            Spacer()
            Button("关闭") {
                onCancel()
                dismiss()
            }
            .buttonStyle(ConsoleSecondaryButtonStyle())
            .keyboardShortcut(.cancelAction)
        }
        .padding(.horizontal, ConsoleTheme.Sheet.padding)
        .padding(.vertical, 14)
    }

    private func point(
        icon: String,
        title: String,
        detail: String,
        tone: SettingsTone = .neutral
    ) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon)
                .font(.system(size: 12))
                .foregroundStyle(iconColor(tone))
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(ConsoleType.rowTitle)
                    .foregroundStyle(ConsoleTheme.text)
                Text(detail)
                    .font(ConsoleType.small)
                    .foregroundStyle(ConsoleTheme.secondaryText)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func iconColor(_ tone: SettingsTone) -> Color {
        switch tone {
        case .warning: return ConsoleTheme.warning
        case .error: return ConsoleTheme.danger
        case .ok: return ConsoleTheme.success
        case .accent: return ConsoleTheme.accent
        case .neutral: return ConsoleTheme.secondaryText
        }
    }
}
