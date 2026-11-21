import SwiftUI

/// 设置中心：左侧分类导航 + 右侧内容。
///
/// 生命周期（只读加载与 `onChange` 副作用）全部挂在这一层，分类切换只换内容，
/// 所以：
/// - 切分类不会重新起助手、不会重新生成配对码、不会改监听范围；
/// - `bridgeEnabled` / `bridgeLANEnabled` / `magpiePort` / Widget 的监听在四个
///   分类下都保持有效（旧实现只有一个 Form，拆页后如果把 onChange 放进分页里，
///   用户切走再改开关就再也不会生效）。
struct SettingsView: View {
    @ObservedObject var model: SettingsModel

    @EnvironmentObject private var settings: AppSettings

    private var categoryBinding: Binding<SettingsCategory> {
        Binding(get: { model.category }, set: { model.selectCategory($0) })
    }

    var body: some View {
        shell
            .modifier(SettingsDialogsModifier(model: model, settings: settings))
            .onAppear { Task { await model.loadOnce() } }
            .onChange(of: settings.bridgeEnabled) { _, enabled in
                model.handle(.bridgeEnabled(enabled))
            }
            .onChange(of: settings.bridgeLANEnabled) { _, enabled in
                model.handle(.bridgeLAN(enabled, bridgeEnabled: settings.bridgeEnabled))
            }
            .onChange(of: settings.magpiePort) { _, _ in
                model.handle(.magpiePort(bridgeEnabled: settings.bridgeEnabled))
            }
            .onChange(of: settings.widgetCards) { _, _ in model.handle(.widgetDisplay) }
            .onChange(of: settings.widgetProvider) { _, _ in model.handle(.widgetDisplay) }
            .onChange(of: settings.widgetSelection) { _, _ in model.handle(.widgetDisplay) }
    }

    private var shell: some View {
        GeometryReader { proxy in
            let compact = proxy.size.width < SettingsWindowMetrics.compactBreakpoint
            if compact {
                VStack(spacing: 0) {
                    SettingsCompactCategoryPicker(selection: categoryBinding)
                    Divider()
                    content
                }
            } else {
                HStack(spacing: 0) {
                    SettingsCategoryNavigation(selection: categoryBinding)
                    Divider()
                    content
                }
            }
        }
        .background(ConsoleTheme.canvas)
        .frame(
            minWidth: SettingsWindowMetrics.minWidth,
            maxWidth: .infinity,
            minHeight: SettingsWindowMetrics.minHeight,
            maxHeight: .infinity
        )
    }

    private var content: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: ConsoleTheme.sectionSpacing) {
                section
            }
            .padding(.horizontal, SettingsWindowMetrics.contentPadding)
            .padding(.top, 20)
            .padding(.bottom, 28)
            .frame(maxWidth: SettingsWindowMetrics.contentMaxWidth, alignment: .leading)
            // 宽窗口把内容整块居中：只加留白，不拉长行宽。
            .frame(maxWidth: .infinity, alignment: .center)
        }
    }

    @ViewBuilder private var section: some View {
        switch model.category {
        case .general:
            GeneralSettingsSection(model: model)
        case .connection:
            ConnectionSettingsSection(model: model)
        case .credentials:
            CredentialSettingsSection(model: model)
        case .advanced:
            AdvancedSettingsSection(model: model)
        }
    }
}
