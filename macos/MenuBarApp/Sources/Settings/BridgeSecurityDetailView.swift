import SwiftUI

/// 设备安全详情：证书与配对凭据。
///
/// 主页只留一行 HTTPS 状态；指纹、有效期、重置与撤销都在这里，
/// 配对时打开就能复制核对，不占用日常设置的每一屏。
struct BridgeSecurityDetailView: View {
    @ObservedObject var model: SettingsModel
    let onClose: () -> Void

    @Environment(\.dismiss) private var dismiss

    private var tls: BridgeTlsStatus? { model.worker.bridgeTls }
    private var certificateStatus: SettingsCertificateStatus? { model.tlsPhase.certificateStatus }
    private var lanServing: Bool { model.server.lanServing }
    private var lanEnabled: Bool { model.settings.bridgeLANEnabled }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()

            ScrollView {
                VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
                    certificateGroup
                    dangerGroup
                }
                .padding(.horizontal, ConsoleTheme.Sheet.padding)
                .padding(.vertical, 16)
                .frame(maxWidth: .infinity, alignment: .center)
            }
            .frame(maxHeight: .infinity)
        }
        .background(ConsoleTheme.canvas)
        .frame(width: 520, height: 520)
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            Text("设备安全")
                .font(ConsoleType.detailTitle)
                .foregroundStyle(ConsoleTheme.text)
            Spacer()
            Button("关闭") { close() }
                .buttonStyle(ConsoleSecondaryButtonStyle())
                .keyboardShortcut(.cancelAction)
        }
        .padding(.horizontal, ConsoleTheme.Sheet.padding)
        .padding(.vertical, 14)
    }

    private var certificateGroup: some View {
        SettingsGroup(title: "证书") {
            SettingsValueRow(
                title: "HTTPS 服务",
                value: certificateStatus?.value ?? httpsText,
                tone: certificateStatus?.tone ?? httpsTone
            )
            SettingsRowDivider()

            if let tls {
                if let expiry = tls.notAfter, !expiry.isEmpty {
                    SettingsValueRow(title: "有效期", value: expiry)
                    SettingsRowDivider()
                }

                VStack(alignment: .leading, spacing: 6) {
                    HStack {
                        Text("证书指纹")
                            .font(ConsoleType.rowTitle)
                            .foregroundStyle(ConsoleTheme.text)
                        Spacer()
                        Button("复制") { model.copyFingerprint() }
                            .buttonStyle(ConsoleSecondaryButtonStyle())
                    }
                    Text(tls.groupedFingerprint)
                        .font(ConsoleType.monoSmall)
                        .foregroundStyle(ConsoleTheme.secondaryText)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                    if let stale = certificateStatus?.fingerprintNotServing {
                        Text(stale)
                            .font(ConsoleType.micro)
                            .foregroundStyle(ConsoleTheme.warning)
                            .fixedSize(horizontal: false, vertical: true)
                    } else {
                        Text("墨水屏上显示的指纹必须与这段完全一致。")
                            .font(ConsoleType.micro)
                            .foregroundStyle(ConsoleTheme.tertiaryText)
                    }
                }
                .padding(.vertical, 8)
            } else {
                SettingsValueRow(
                    title: "证书",
                    value: "未准备",
                    detail: lanEnabled ? "桥接启动时会自动准备。" : nil
                )
            }

            // 身份没换成功：服务状态仍是真实的，错误单独说清楚。
            if let notice = model.tlsPhase.notice, certificateStatus == nil {
                SettingsInlineNotice(
                    kind: notice.tone == .error ? .error : .warning,
                    message: notice.text
                )
                .padding(.bottom, 8)
            }

            if let error = model.worker.bridgeTlsError {
                SettingsInlineNotice(kind: .error, message: error)
                    .padding(.bottom, 8)
            }

            if let notice = model.securityNotice {
                SettingsInlineNotice(kind: .info, message: notice)
                    .padding(.bottom, 8)
            }
        }
    }

    private var dangerGroup: some View {
        SettingsGroup(title: "身份与配对") {
            SettingsDangerRow(
                title: "重置证书身份",
                detail: "换掉根证书，所有已配对设备都要重新确认新指纹。",
                buttonTitle: "重置…",
                disabled: model.tlsPhase.isBusy
            ) {
                model.requestTLSReset()
            }

            SettingsRowDivider()

            SettingsDangerRow(
                title: "撤销已配对设备",
                detail: "旧配对凭证立即失效，每台设备都要重新配对。",
                buttonTitle: "撤销…",
                disabled: model.busy
            ) {
                model.confirmRevokeDevices = true
            }
        }
    }

    private var httpsText: String {
        if lanServing { return "运行中（TLS）" }
        if lanEnabled { return "未启动" }
        return "仅本机"
    }

    private var httpsTone: SettingsTone {
        if lanServing { return .ok }
        if lanEnabled { return .warning }
        return .neutral
    }

    private func close() {
        onClose()
        dismiss()
    }
}
