import AppKit
import SwiftUI

/// Offscreen renderer for the fixed SwiftUI cards and before/after screenshots.
/// Requires a macOS GUI session.

@MainActor
@main
struct ScreenshotHarness {
    static func main() {
        let output = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "/tmp/aicc-screenshots"
        try? FileManager.default.createDirectory(atPath: output, withIntermediateDirectories: true)
        let settings = AppSettings.shared

        let workbuddyData = WorkBuddyData(
            points: 5343.37,
            used_points: 126,
            total_points: 10_000,
            reset_text: "Reset 2026-07-31 23:59",
            balance_state: "Connected",
            balance_stale: false,
            balance_updated_at: "2026-07-31 19:19:50",
            balance_updated_epoch: nil,
            balance_age_seconds: nil,
            balance_error_code: nil,
            balance_error: nil,
            auto_used_credits: 126,
            usage_records: nil,
            usage_source: "daemon-rpc"
        )
        let deepseekData = DeepSeekData(
            status: "Online",
            balances: [DeepSeekBalance(currency: "CNY", total_balance: "128.50", granted_balance: "0", topped_up_balance: "128.50")],
            usage: [DeepSeekUsage(currency: "CNY", used_today: "2.34")],
            source: "Observed balance"
        )
        let codexData = CodexData(
            five_hour: RateWindow(remaining: 65, reset: "14:27", label: nil, duration_minutes: nil),
            weekly: RateWindow(remaining: 92, reset: "7月17日", label: nil, duration_minutes: nil),
            source: "Codex app-server",
            state: "Connected",
            stale: false,
            available: true
        )
        let systemData = SystemData(
            label: "System",
            status: "Online",
            cpu: "18%",
            platform: "macOS",
            ram: "8.2 GB",
            gpu: nil
        )

        let cardWidth: CGFloat = 322
        let miniWidth: CGFloat = 156

        render(CodexCard(codex: codexData).environmentObject(settings), name: "legacy-codex-card", width: cardWidth, scheme: .aqua, to: output)
        render(WorkBuddyCard(data: workbuddyData).environmentObject(settings), name: "legacy-workbuddy-card", width: miniWidth, scheme: .aqua, to: output)
        render(DeepSeekCard(data: deepseekData).environmentObject(settings), name: "legacy-deepseek-card", width: miniWidth, scheme: .aqua, to: output)
        render(SystemCard(data: systemData).environmentObject(settings), name: "system-card", width: cardWidth, scheme: .aqua, to: output)

        // Reconstructed pre-P1 typography for before/after comparison.
        render(LegacyCompactCard(title: "WorkBuddy", icon: "wand.and.stars", value: "5,343.37", subtitle: "Connected · 19:19:50", state: .online), name: "before-workbuddy", width: miniWidth, scheme: .aqua, to: output)
        render(LegacyCompactCard(title: "DeepSeek", icon: "brain.head.profile", value: "¥128.50", subtitle: "Online · 今日 ¥2.34", state: .online), name: "before-deepseek", width: miniWidth, scheme: .aqua, to: output)
        render(LegacyCodexCard(weekly: 92, fiveHour: 65, weeklyReset: "7月17日"), name: "before-codex", width: cardWidth, scheme: .aqua, to: output)

        print("Screenshots written to \(output)")
    }

    @MainActor
    static func render<V: View>(
        _ view: V,
        name: String,
        width: CGFloat,
        scheme: NSAppearance.Name,
        to output: String
    ) {
        let background: Color = scheme == .darkAqua
            ? Color(red: 0.10, green: 0.10, blue: 0.12)
            : Color(red: 0.97, green: 0.97, blue: 0.96)
        let wrapped = view
            .padding(20)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(background)
        let hosting = NSHostingView(rootView: wrapped)
        let fitting = hosting.fittingSize
        let renderWidth = max(width, fitting.width)
        let height = max(140, fitting.height)
        let scale: CGFloat = 2
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: renderWidth, height: height),
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        window.contentView = hosting
        window.appearance = NSAppearance(named: scheme)
        window.orderFront(nil)
        hosting.frame = window.contentView?.bounds ?? hosting.frame
        hosting.layoutSubtreeIfNeeded()
        guard let rep = hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds) else {
            print("WARN: could not render \(name)")
            window.orderOut(nil)
            return
        }
        rep.size = NSSize(width: renderWidth * scale, height: height * scale)
        hosting.cacheDisplay(in: hosting.bounds, to: rep)
        window.orderOut(nil)
        guard let data = rep.representation(using: .png, properties: [:]) else {
            print("WARN: could not encode \(name)")
            return
        }
        let path = "\(output)/\(name).png"
        do {
            try data.write(to: URL(fileURLWithPath: path))
            print("wrote \(path)")
        } catch {
            print("WARN: could not write \(path): \(error)")
        }
    }
}

/// Reconstructed pre-P1 compact card (small 20pt value, no unit split).
struct LegacyCompactCard: View {
    let title: String
    let icon: String
    let value: String
    let subtitle: String
    let state: CardState

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 4) {
                Image(systemName: icon).font(.system(size: 10)).foregroundColor(.secondary)
                Text(title).font(.system(size: 10, weight: .medium)).foregroundColor(.secondary)
                Spacer()
                Circle().fill(state == .online ? Color.green : Color.secondary).frame(width: 5, height: 5)
            }
            Text(value)
                .font(.system(size: 20, weight: .semibold))
                .lineLimit(1)
                .minimumScaleFactor(0.55)
            Text(subtitle)
                .font(.system(size: 9))
                .foregroundColor(.secondary)
                .lineLimit(1)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 10).fill(Color.primary.opacity(0.04)))
    }
}

/// Reconstructed pre-P1 Codex card (26pt inline percentage, thin bar).
struct LegacyCodexCard: View {
    let weekly: Double
    let fiveHour: Double
    let weeklyReset: String

    var body: some View {
        VStack(spacing: 8) {
            HStack(alignment: .lastTextBaseline) {
                Text("Codex Weekly").font(.system(size: 11, weight: .medium)).foregroundColor(.secondary)
                Spacer()
                Text(String(format: "%.0f%%", weekly)).font(.system(size: 26, weight: .semibold)).foregroundColor(.green)
                Text("left").font(.system(size: 11)).foregroundColor(.secondary)
            }
            GeometryReader { geo in
                ZStack(alignment: .leading) {
                    RoundedRectangle(cornerRadius: 2).fill(Color.primary.opacity(0.1)).frame(height: 4)
                    RoundedRectangle(cornerRadius: 2).fill(Color.green).frame(width: geo.size.width * CGFloat(weekly / 100), height: 4)
                }
            }
            .frame(height: 4)
            HStack {
                Spacer()
                Text("5h: \(String(format: "%.0f", fiveHour))% · 重置 \(weeklyReset)")
                    .font(.system(size: 10))
                    .foregroundColor(.secondary)
            }
        }
        .frame(maxWidth: .infinity)
    }
}
