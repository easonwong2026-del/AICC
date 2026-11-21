import SwiftUI

/// 设置窗口的弹层与确认框。
///
/// 单独抽出来有两个原因：
/// 1. 所有确认文案集中在一处，便于逐条核对"危险操作没有丢确认"；
/// 2. `body` 里的修饰符链过长会让类型检查超时。
struct SettingsDialogsModifier: ViewModifier {
    @ObservedObject var model: SettingsModel
    @ObservedObject var settings: AppSettings

    func body(content: Content) -> some View {
        content
            .sheet(isPresented: $model.showQuotaSelection) {
                QuotaSelectionView(
                    port: settings.magpiePort,
                    selection: $settings.widgetSelection
                ) { picked in
                    settings.widgetSelection = picked
                }
            }
            .sheet(isPresented: $model.showCredentialRisk) {
                PlaintextCredentialSheet(
                    risk: model.worker.credentialStorage?.risk ?? "",
                    onConfirm: { await model.setJSONCredentialStorage(true) },
                    onCancel: {}
                )
            }
            .sheet(isPresented: $model.showBridgeSecurity) {
                BridgeSecurityDetailView(model: model) {
                    model.showBridgeSecurity = false
                }
            }
            .sheet(isPresented: $model.showCredentialDetail) {
                CredentialMigrationDetailView(worker: model.worker) {
                    model.showCredentialDetail = false
                }
            }
            .confirmationDialog(
                "之后新建的授权会重新使用钥匙串",
                isPresented: $model.confirmDisableJSON
            ) {
                Button("改回钥匙串") {
                    Task { _ = await model.setJSONCredentialStorage(false) }
                }
                Button("取消", role: .cancel) {}
            } message: {
                Text("已经写成 JSON 的授权不受影响，继续可用；不会自动迁移回钥匙串。")
            }
            .confirmationDialog(
                "撤销设备后需要重新配对",
                isPresented: $model.confirmRevokeDevices
            ) {
                Button("撤销所有设备", role: .destructive) {
                    Task { await model.revokePairedDevices() }
                }
                Button("取消", role: .cancel) {}
            } message: {
                Text("所有已配对的墨水屏与 Android 设备会立即失去访问权限，需要重新生成配对码。")
            }
            .confirmationDialog(
                "重置证书身份？",
                isPresented: Binding(
                    get: { model.tlsPhase.isConfirming },
                    set: { presented in if !presented { model.cancelTLSReset() } }
                )
            ) {
                Button("重置证书身份", role: .destructive) {
                    Task { await model.confirmTLSReset() }
                }
                Button("取消", role: .cancel) { model.cancelTLSReset() }
            } message: {
                Text("会换掉根证书：所有已配对设备都必须重新确认新指纹，当前的配对码立即失效。执行期间请勿关闭 AICC。")
            }
    }
}
