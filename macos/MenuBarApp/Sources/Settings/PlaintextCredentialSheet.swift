import SwiftUI

/// 首次启用明文 JSON 的风险说明与确认（CRED-3-UI）。
///
/// 只在用户**主动**点「启用本地 JSON」时出现。点「仍使用钥匙串」什么都不写 ——
/// 助手的默认行为本来就是钥匙串，所以拒绝是零成本的。
struct PlaintextCredentialSheet: View {
    /// 助手给出的风险说明（`health.credentials.risk`）。为空时用本地文案兜底。
    let risk: String
    /// 返回是否保存成功；失败时留在本页并显示原因。
    let onConfirm: () async -> Bool
    let onCancel: () -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var busy = false
    @State private var failure: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(alignment: .firstTextBaseline) {
                Label("启用本地 JSON 存储", systemImage: "lock.open.display")
                    .font(.headline)
                Spacer()
                Button("关闭") {
                    onCancel()
                    dismiss()
                }
                .keyboardShortcut(.cancelAction)
            }
            .padding(.horizontal, 20)
            .padding(.top, 18)

            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    // 不用 `**加粗**`：这里渲染出来是字面量，`Text` 只在部分上下文里解析 markdown。
                    Text("AICC 可以把之后新建的 WorkBuddy 与 Qoder 自有授权（设备授权、PAT）保存为本机 JSON 文件，不再写入 macOS 钥匙串。这样后台任务不会因为 App 重新构建、签名变化而反复弹出授权框。")
                        .fixedSize(horizontal: false, vertical: true)

                    point(
                        icon: "doc.plaintext",
                        title: "凭据是明文保存的",
                        detail: risk.isEmpty
                            ? "凭据目录是 0700、文件是 0600，但这只能挡住同机的其他用户，挡不住同一个 macOS 用户下运行的其他程序。"
                            : risk
                    )
                    point(
                        icon: "externaldrive.badge.exclamationmark",
                        title: "建议开启 FileVault",
                        detail: "不要把凭据目录放进云同步（iCloud、Dropbox 等），也不要把明文 JSON 放进未加密的备份或诊断包。"
                    )
                    point(
                        icon: "clock.arrow.circlepath",
                        title: "只影响之后新建的授权",
                        detail: "已经登录的账号（含 Qoder）保持原来的存储方式，不会因为打开这个开关而自动迁移；把旧账号搬到本地文件是后续版本里的显式操作。"
                    )
                    point(
                        icon: "arrow.uturn.backward",
                        title: "随时可以改回去",
                        detail: "在「设置 → 凭据存储」里关闭后，之后新建的授权重新使用钥匙串；已经写成 JSON 的授权不会因此失效。"
                    )
                    point(
                        icon: "key",
                        title: "钥匙串仍会用到",
                        detail: "Qoder 客户端自身的登录凭据、以及你主动导入时读取的第三方钥匙串条目不受这个开关影响；再打开这个开关之前保存的 AICC 授权也仍在钥匙串里。"
                    )

                    if let failure {
                        Text(failure)
                            .font(.callout)
                            .foregroundStyle(.red)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                .padding(.horizontal, 20)
                .padding(.top, 14)
                .padding(.bottom, 16)
            }

            Divider()

            HStack {
                Button("仍使用钥匙串") {
                    onCancel()
                    dismiss()
                }
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
                .keyboardShortcut(.defaultAction)
                .disabled(busy)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 14)
        }
        .frame(width: 560, height: 520)
    }

    private func point(icon: String, title: String, detail: String) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon)
                .foregroundStyle(.secondary)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 3) {
                Text(title).font(.callout.weight(.semibold))
                Text(detail)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}
