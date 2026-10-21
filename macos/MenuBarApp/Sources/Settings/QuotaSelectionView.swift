import SwiftUI

/// Picks which quota windows the widgets draw.  A row is one window of one
/// account; the order of the ticks is the order on screen.  Ticking nothing
/// means "decide for me", which is what the widget did before this screen.
struct QuotaSelectionView: View {
    let port: Int
    @Binding var selection: [String]
    let onApply: ([String]) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var snapshot: QuotaSnapshot?
    @State private var loading = false
    @State private var failure: String?
    @State private var picked: [String] = []

    private struct Row: Identifiable {
        let key: String
        let provider: String
        let account: String
        let window: String
        let value: String
        var id: String { key }
    }

    private var rows: [Row] {
        guard let snapshot else { return [] }
        return snapshot.accounts.flatMap { card in
            card.items.map { item in
                Row(key: card.key(for: item), provider: card.providerName, account: card.accountLabel,
                    window: item.name, value: item.primaryText)
            }
        }
    }

    private var grouped: [(provider: String, rows: [Row])] {
        var order: [String] = []
        var byProvider: [String: [Row]] = [:]
        for row in rows {
            if byProvider[row.provider] == nil { order.append(row.provider) }
            byProvider[row.provider, default: []].append(row)
        }
        return order.map { ($0, byProvider[$0] ?? []) }
    }

    private var missingSelection: Int {
        let available = Set(rows.map(\.key))
        return picked.filter { !available.contains($0) }.count
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("选择显示的额度")
                    .font(.headline)
                Spacer()
                Button("关闭") { dismiss() }
            }
            .padding(.horizontal, 18)
            .padding(.top, 16)

            Text("勾选要显示的额度窗口，顺序即显示顺序。不勾选＝自动按 Magpie 顺序显示。")
                .font(.caption)
                .foregroundStyle(.secondary)
                .padding(.horizontal, 18)
                .padding(.top, 4)

            Group {
                if loading {
                    ProgressView("读取 Magpie 额度…").frame(maxWidth: .infinity, maxHeight: .infinity)
                } else if let failure {
                    VStack(spacing: 8) {
                        Text(failure).foregroundStyle(.secondary)
                        Button("重试") { Task { await load() } }
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else if rows.isEmpty {
                    Text("Magpie 暂无额度，先在 Magpie 登录服务商。")
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                } else {
                    List {
                        if !picked.isEmpty {
                            Section {
                                Button("清空选择（恢复自动）") { picked = [] }
                            }
                        }
                        ForEach(grouped, id: \.provider) { group in
                            Section(group.provider) {
                                ForEach(group.rows) { row in
                                    Button {
                                        toggle(row.key)
                                    } label: {
                                        HStack(spacing: 8) {
                                            Image(systemName: picked.contains(row.key) ? "checkmark.square.fill" : "square")
                                                .foregroundStyle(picked.contains(row.key) ? Color.accentColor : Color.secondary)
                                            VStack(alignment: .leading, spacing: 1) {
                                                Text(row.window).font(.callout)
                                                Text(row.account).font(.caption2).foregroundStyle(.secondary)
                                            }
                                            Spacer()
                                            Text(row.value).font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                                            if let index = picked.firstIndex(of: row.key) {
                                                Text("\(index + 1)")
                                                    .font(.caption2.monospacedDigit())
                                                    .foregroundStyle(.secondary)
                                            }
                                        }
                                    }
                                    .buttonStyle(.plain)
                                }
                            }
                        }
                    }
                }
            }
            .frame(maxHeight: .infinity)

            if missingSelection > 0 {
                Text("有 \(missingSelection) 项在 Magpie 里已不存在（窗口改名或账号移除），应用时会自动去掉。")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 18)
            }

            Divider()
            HStack {
                Text(picked.isEmpty ? "当前：自动" : "当前：已选 \(picked.count) 项")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Spacer()
                Button("取消") { dismiss() }
                Button("应用") {
                    let available = Set(rows.map(\.key))
                    onApply(picked.filter { available.contains($0) })
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
            }
            .padding(.horizontal, 18)
            .padding(.vertical, 12)
        }
        .frame(width: 460, height: 520)
        .task { await load() }
        .onAppear { picked = selection }
    }

    private func toggle(_ key: String) {
        if let index = picked.firstIndex(of: key) {
            picked.remove(at: index)
        } else {
            picked.append(key)
        }
    }

    private func load() async {
        loading = true
        failure = nil
        defer { loading = false }
        let snapshot = await WidgetStatusLoader.snapshot(port: port)
        if snapshot.accounts.isEmpty {
            failure = "没有读到额度。确认 Magpie 正在运行（端口 \(port)）。"
        } else {
            self.snapshot = snapshot
        }
    }
}
