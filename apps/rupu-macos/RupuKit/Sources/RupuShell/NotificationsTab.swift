import SwiftUI
import AppKit
import RupuStore
import RupuDesign

/// The Settings **Notifications** tab (Phase 6A, Task 7) — three per-kind
/// toggles bound directly to `RunNotifier`'s `@Observable` prefs, plus an
/// authorization-denied banner (shown once a `requestAuthorization()` call
/// has come back denied) with a deep link to this app's System Settings
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
        Form {
            if notifier.authorizationDenied {
                authorizationDeniedBanner
                    .padding(.bottom, 4)
            }

            Section("Notify me when") {
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
                    title: "A run completes",
                    detail: "Every run that finishes — successful or not.",
                    isOn: $notifier.notifyCompletions
                )
            }
        }
        .padding(.top, 12)
        // The banner (when shown) is a single row above a three-row
        // Section; without a floor this can visually collapse thinner than
        // the other three tabs when it's absent, same rationale as
        // `SettingsView.generalTab`'s own `minHeight`.
        .frame(minHeight: 200, alignment: .top)
    }

    private func toggleRow(title: String, detail: String, isOn: Binding<Bool>) -> some View {
        Toggle(isOn: isOn) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
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
