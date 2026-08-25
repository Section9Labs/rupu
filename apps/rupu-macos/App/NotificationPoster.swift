import Foundation
import RupuStore
import UserNotifications
import os

/// Prod `NotificationPosting` — the only thing in this arc allowed to touch
/// `UNUserNotificationCenter`. Lives in the App target, not `RupuStore`,
/// specifically so nothing outside a real app bundle can construct one:
/// `UNUserNotificationCenter` throws under `swift test` (no bundle
/// context), and `RunNotifier.init`'s `poster` parameter has no default, so
/// every call site — including every test target, which can't even import
/// this file — must explicitly choose an implementation. Only `RupuApp`
/// names this type.
///
/// Stateless: every call reads `UNUserNotificationCenter.current()` fresh,
/// so there's nothing here to share or leak across calls.
struct UNCenterNotificationPoster: NotificationPosting {
    private static let logger = Logger(subsystem: "com.section9labs.rupu", category: "notifications")

    func requestAuthorization() async -> Bool {
        do {
            return try await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound])
        } catch {
            // A throw here is NOT the user declining (that's a normal
            // `false` return) — it's the OS refusing to register the app at
            // all, most commonly because the bundle is unsigned ("notifications
            // are not allowed for this application"; the reason macos-build
            // ad-hoc signs). Silently mapping it to `false` once masked
            // exactly that for every `make macos-run` build.
            Self.logger.error("notification authorization request failed: \(error, privacy: .public)")
            return false
        }
    }

    func post(_ content: NotificationContent) async {
        let payload = UNMutableNotificationContent()
        payload.title = content.title
        payload.body = content.body
        payload.sound = .default
        payload.userInfo = ["runID": content.runID]
        let request = UNNotificationRequest(identifier: UUID().uuidString, content: payload, trigger: nil)
        do {
            try await UNUserNotificationCenter.current().add(request)
        } catch {
            Self.logger.error("posting notification failed: \(error, privacy: .public)")
        }
    }

    func currentAuthorizationStatus() async -> NotificationAuthorizationStatus {
        let status = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
        switch status {
        case .authorized, .provisional, .ephemeral:
            return .authorized
        case .denied:
            return .denied
        case .notDetermined:
            return .notDetermined
        @unknown default:
            // A future case this SDK doesn't know about yet — treat it as
            // "haven't asked" rather than risk showing a false "denied"
            // banner over a status we can't actually interpret.
            return .notDetermined
        }
    }
}
