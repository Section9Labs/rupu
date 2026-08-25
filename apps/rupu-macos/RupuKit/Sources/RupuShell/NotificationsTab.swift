import SwiftUI
import AppKit
import RupuStore
import RupuDesign

/// The Settings **Notifications** tab (Phase 6A, Task 7) — three per-kind
/// toggles bound directly to `RunNotifier`'s `@Observable` prefs, plus an
/// authorization-denied banner (shown while `notifier.authorizationDenied`
/// is `true`, kept in sync by `syncAuthorizationStatus()` — see this view's
/// own `.task` below) with a deep link to this app's System Settings
/// notifications pane.
///
/// `notifier` is owned by `RupuApp`, not this view — toggling a pref here is
/// the same live instance already wired to `RunNotifier.activate`'s firehose
/// consumer, not a disconnected copy.
public struct NotificationsTab: View {
    @Bindable var notifier: RunNotifier

    public init(notifier: RunNotifier) {
        self.notifier = notifier
    }

    public var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            if notifier.authorizationDenied {
                authorizationDeniedBanner
            }

            // Redesign-pass fix (spec §4, "Settings scene tone"): this tab
            // used to be a native `Form`/`Section`, painting the same
            // chrome-gray material the audit (A8) flagged across every
            // Settings tab. A token-styled panel — `Eyebrow` header over a
            // `Color.rupuPanel` card, the exact idiom `SettingsView`'s own
            // `settingsCard`/`ConfigTab`'s `sectionCard` already use — keeps
            // the native `Toggle` controls but retones the surface around
            // them.
            VStack(alignment: .leading, spacing: 10) {
                Eyebrow("Notify me when")
                VStack(alignment: .leading, spacing: 12) {
                    toggleRow(
                        title: "A run needs approval",
                        detail: "A gate step is waiting on you before it can continue.",
                        isOn: $notifier.notifyGates
                    )
                    toggleRow(
                        title: "A step or run fails",
                        detail: "Any step error, or a run that ends in a failed state.",
                        isOn: $notifier.notifyFailures
                    )
                    toggleRow(
                        title: "A run finishes",
                        detail: "Completed, cancelled, or rejected. Failed runs are covered by Failures.",
                        isOn: $notifier.notifyCompletions
                    )
                }
            }
            .padding(12)
            .frame(maxWidth: .infinity, alignment: .leading)
            .panelStyle(.panel)

            Spacer(minLength: 0)
        }
        .padding(.top, 12)
        // The banner (when shown) is a single row above the three-toggle
        // panel; without a floor this can visually collapse thinner than
        // the other three tabs when it's absent, same rationale as
        // `SettingsView.generalTab`'s own `minHeight`.
        .frame(minHeight: 200, alignment: .top)
        .task {
            // Non-prompting: syncs `authorizationDenied` against the OS's
            // actual current status every time this tab is shown, catching
            // a status the user changed directly in System Settings since
            // the last sync (either direction — a stale `true` banner OR a
            // stale `false` that should now show one).
            await notifier.syncAuthorizationStatus()
        }
    }

    private func toggleRow(title: String, detail: String, isOn: Binding<Bool>) -> some View {
        Toggle(isOn: isOn) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.uiText)
                    .foregroundStyle(Color.rupuInk)
                Text(detail)
                    .font(.noteText)
                    .foregroundStyle(Color.rupuMute)
            }
        }
    }

    private var authorizationDeniedBanner: some View {
        TintBanner(tone: .rupuWarn, toneBg: .rupuWarnBg) {
            HStack(alignment: .top, spacing: 8) {
                Icon(.shieldAlert, size: 14)
                    .foregroundStyle(Color.rupuWarn)
                VStack(alignment: .leading, spacing: 6) {
                    Text("Notifications are turned off for rupu.app in System Settings — these toggles record your preference, but nothing will actually appear until you re-enable them there.")
                        .font(.uiText)
                        .foregroundStyle(Color.rupuInk)
                    Button("Open System Settings") {
                        guard let url = URL(string: "x-apple.systempreferences:com.apple.preference.notifications") else { return }
                        NSWorkspace.shared.open(url)
                    }
                    .font(.noteText)
                }
                Spacer(minLength: 0)
            }
        }
    }
}
