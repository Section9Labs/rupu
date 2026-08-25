import Foundation
import RupuStore
import UserNotifications

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
    func requestAuthorization() async -> Bool {
        (try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound])) ?? false
    }

    func post(_ content: NotificationContent) async {
        let payload = UNMutableNotificationContent()
        payload.title = content.title
        payload.body = content.body
        payload.sound = .default
        payload.userInfo = ["runID": content.runID]
        let request = UNNotificationRequest(identifier: UUID().uuidString, content: payload, trigger: nil)
        try? await UNUserNotificationCenter.current().add(request)
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
