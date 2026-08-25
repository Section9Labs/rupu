import SwiftUI

/// The one failed-lazy-block rendering every screen shares — the `TintBanner`
/// err-tone chrome the per-screen `failedBlock`/`securityFailedBlock`/
/// `usageFailedBlock`/`FailedNote` copies used to carry, plus the retry
/// affordance none of them had: an outline Retry button wired to the block's
/// own reload. `retry` is required, not optional — every failed block has a
/// real reload path (a store's `loadX()` force twin or `activate()`), and an
/// unretriable failure banner is exactly the dead end this type exists to
/// remove.
///
/// The button disables itself while the retry is in flight: some stores'
/// force reloads (`SecurityStore.loadFindings`, `LibraryStore.loadAgents`)
/// deliberately leave the block `.failed` until the refetch resolves rather
/// than flashing back to `.loading`, so without this guard a slow retry
/// would look like a dead button and invite double-fires. When the reload
/// resolves, the block's `BlockState` moves off `.failed` and this view
/// disappears wholesale — no local state to clean up.
public struct FailedBlock: View {
    private let subject: String
    private let message: String
    private let retry: () async -> Void

    @State private var isRetrying = false

    public init(subject: String, message: String, retry: @escaping () async -> Void) {
        self.subject = subject
        self.message = message
        self.retry = retry
    }

    public var body: some View {
        TintBanner(tone: Color.status(.failed), toneBg: Color.status(.failed).opacity(0.08)) {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Failed to load \(subject)")
                        .font(.noteText.weight(.semibold))
                        .foregroundStyle(Color.status(.failed))
                    Text(message)
                        .font(.noteText)
                        .foregroundStyle(Color.status(.failed))
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Button("Retry") {
                    isRetrying = true
                    Task {
                        await retry()
                        isRetrying = false
                    }
                }
                .buttonStyle(RupuButtonStyle.outline)
                .disabled(isRetrying)
                .opacity(isRetrying ? 0.5 : 1)
            }
        }
    }
}
