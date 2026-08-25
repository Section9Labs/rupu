import AppKit
import SwiftUI
import RupuShell
import RupuStore
import RupuDesign
import UserNotifications
import os

/// Routes app termination through `BackendController.shutdown` on both
/// paths that can end this process:
///
/// - AppKit-graceful (Cmd-Q / Dock Quit / File > Quit) calls
///   `applicationShouldTerminate`; returning `.terminateLater` and replying
///   only after `shutdown` completes is what makes the async teardown
///   actually finish before the process exits — a plain
///   `NSApplication.willTerminateNotification` observer fires without
///   blocking termination, so an async `Task` started from it can be cut
///   off mid-`killpg`.
/// - A bare `kill -TERM <pid>` (used by the Task 9 smoke test, since the
///   display may be locked and Cmd-Q isn't clickable) bypasses AppKit's
///   termination machinery entirely — SIGTERM's default disposition just
///   kills the process, no delegate callback involved. The installed
///   `DispatchSourceSignal` runs `backend.shutdown` itself and exits
///   directly, deliberately **not** funneling through
///   `NSApplication.terminate(nil)`: routing it through `terminate(nil)` →
///   `.terminateLater` was measured (headless, screen-locked launch, no
///   real window ever became key) to leave the `Task { @MainActor in }`
///   inside `applicationShouldTerminate` permanently unscheduled — AppKit's
///   termination wait appears to pump a run-loop mode that starves Swift
///   Concurrency's MainActor executor in that launch shape. Since a bare
///   `kill -TERM` already bypasses AppKit's delegate machinery by
///   definition, there's no reply/veto contract to honor here — doing the
///   shutdown directly and calling `exit(0)` sidesteps the hazard rather
///   than working around it. `applicationShouldTerminate` keeps the
///   standard API-contract shape below for the interactive Cmd-Q path,
///   which is unverified here (needs a real, unlocked GUI session — see
///   the Task 9 report) but should not share the headless launch's
///   run-loop starvation.
///
/// `BackendController.shutdown`/`EmbeddedServer.stop` are idempotent, so
/// both paths racing is harmless.
///
/// `@MainActor`: AppKit already calls every `NSApplicationDelegate` method
/// on the main thread, but the `UNUserNotificationCenterDelegate` async
/// methods below (`willPresent`/`didReceive`) have no actor annotation of
/// their own — without this, `didReceive` touching `model` (a `@MainActor`
/// type) requires bridging via `MainActor.run`, and under strict
/// concurrency checking that flags "sending self risks data races" (`self`
/// isn't `Sendable`). Isolating the whole class instead is the standard fix
/// for exactly this shape: an `async` delegate requirement with no
/// annotation of its own can be satisfied by a `@MainActor` method without
/// any extra hop.
@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private static let logger = Logger(subsystem: "com.section9labs.rupu", category: "lifecycle")

    var backend: BackendController?
    /// Set from `RupuApp`'s `.onAppear` alongside `backend` — needed so
    /// `userNotificationCenter(_:didReceive:)` below can route a notification
    /// tap through the same `AppModel.navigate(to:)` every other deep-link
    /// in this app uses.
    var model: AppModel?
    private var sigtermSource: DispatchSourceSignal?

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Claims the tap-routing/foreground-presentation delegate slot. This
        // is app-level, real-bundle code — never touched by unit tests
        // (`RunNotifier`'s own `NotificationPosting` seam is what tests
        // exercise instead; see that type's doc comment).
        UNUserNotificationCenter.current().delegate = self
        signal(SIGTERM, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
        source.setEventHandler { [weak self] in
            Self.logger.info("SIGTERM received — shutting down backend directly")
            Task { @MainActor in
                let keepRunning = UserDefaults.standard.bool(forKey: "keepServerRunning")
                await self?.backend?.shutdown(keepRunning: keepRunning)
                Self.logger.info("backend shutdown complete — exiting")
                exit(0)
            }
        }
        source.resume()
        sigtermSource = source
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let backend else { return .terminateNow }
        Self.logger.info("applicationShouldTerminate — shutting down backend before replying")
        Task { @MainActor in
            let keepRunning = UserDefaults.standard.bool(forKey: "keepServerRunning")
            await backend.shutdown(keepRunning: keepRunning)
            Self.logger.info("backend shutdown complete — replying to terminate")
            NSApplication.shared.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }
}

/// Tap routing for `RunNotifier`'s local notifications.
extension AppDelegate: UNUserNotificationCenterDelegate {
    /// `nonisolated`, not `@MainActor` (even though the class itself is):
    /// `UNUserNotificationCenterDelegate`'s methods carry no actor
    /// annotation of their own, and their parameter types (`UNNotification`/
    /// `UNNotificationResponse`/`UNUserNotificationCenter`) aren't
    /// `Sendable` — a `@MainActor` override would require the FRAMEWORK's
    /// caller to send a non-`Sendable` value across the actor boundary,
    /// which strict concurrency checking rejects. `nonisolated` matches the
    /// requirement's own isolation exactly, so nothing needs to cross;
    /// extracting the one `Sendable` piece we actually need (`runID`, a
    /// `String?`) here and handing THAT to a `@MainActor` helper is what
    /// lets the rest of the work reach `model` safely.
    ///
    /// Without `.banner`/`.sound` here, macOS silently swallows a
    /// notification whenever this app is already frontmost — matching the
    /// "toggles describe what they do" honesty bar: an enabled pref that
    /// silently produced nothing whenever the app happened to be focused
    /// would be a lie by omission.
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .sound]
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse
    ) async {
        let runID = response.notification.request.content.userInfo["runID"] as? String
        await routeNotificationTap(runID: runID)
    }

    /// Notifications this app posts only ever describe local-CP runs —
    /// `RunNotifier.activate` is fed by `backend.makeFirehoseStream`, the
    /// LOCAL firehose (never a remote host's), so `host: nil` here is
    /// always correct, not a shortcut.
    private func routeNotificationTap(runID: String?) {
        if let runID {
            model?.navigate(to: .runDetail(id: runID, host: nil))
        }
        NSApp.activate(ignoringOtherApps: true)
        frontMainWindow()
    }

    /// `NSApp.windows.first` is unordered and is NOT guaranteed to be the
    /// main content window — it can just as easily be the Settings window,
    /// or (once it exists) Task 8's menu-bar status window. A notification
    /// tap must always land on the app's real content, so this picks
    /// deliberately: prefer the window carrying `RupuApp`'s own explicit
    /// `WindowGroup(id: "main")` identifier, and if that's ever unavailable
    /// for some reason, fall back to any window that ISN'T the Settings
    /// scene's window (`"com_apple_SwiftUI_Settings_window"` is SwiftUI's
    /// own stable internal identifier for a macOS `Settings { }` scene's
    /// window) — the one hard requirement either way is that the Settings
    /// window itself is never the one fronted.
    private func frontMainWindow() {
        let target = NSApp.windows.first(where: { $0.identifier?.rawValue == RupuApp.mainWindowID })
            ?? NSApp.windows.first(where: { $0.identifier?.rawValue != RupuApp.settingsWindowID })
        target?.makeKeyAndOrderFront(nil)
    }
}

@main
struct RupuApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @State private var model = AppModel()
    @State private var backend = BackendController()
    /// Owned here, not by any screen — a firehose subscriber that must
    /// outlive whatever screen happens to be on-screen. `SettingsView`'s
    /// Notifications tab reads/writes the same instance's prefs; the
    /// `.onChange(of: backend.health)` below is this arc's own activation
    /// seam, independent of `RootView`'s (kept minimal on purpose — see
    /// `RunNotifier.activate`'s doc comment for why re-activating on every
    /// healthy transition is safe).
    @State private var runNotifier = RunNotifier(poster: UNCenterNotificationPoster())
    @AppStorage("appearance") private var appearance: String = "system"

    /// The explicit identifier this app's one `WindowGroup` carries — see
    /// `AppDelegate.frontMainWindow`'s doc comment for why a notification
    /// tap needs to name a specific window rather than trusting
    /// `NSApp.windows.first`'s undefined ordering.
    static let mainWindowID = "main"
    /// SwiftUI's own stable internal identifier for a macOS `Settings { }`
    /// scene's window — not something this app assigns itself, but a
    /// well-known constant `frontMainWindow` excludes by.
    static let settingsWindowID = "com_apple_SwiftUI_Settings_window"

    var body: some Scene {
        WindowGroup(id: RupuApp.mainWindowID) {
            RootView(model: model, backend: backend)
                .frame(minWidth: 1150, minHeight: 760)
                .preferredColorScheme(preferredColorScheme)
                .tint(Color.rupuBrand)
                .onAppear {
                    appDelegate.backend = backend
                    appDelegate.model = model
                }
                .onChange(of: backend.health) { _, newHealth in
                    guard case .healthy = newHealth else { return }
                    // `MainActor.assumeIsolated` here matches
                    // `RunDetailStore.makeRunSignalsFactory`/`OverviewScreen.
                    // makeSignalsFactory`'s own bridging into
                    // `backend.make*Stream` from inside a factory closure —
                    // `streamFactory` is invoked from `RunNotifier.activate`'s
                    // task, which always runs on the main actor (`RunNotifier`
                    // is `@MainActor`), so the assertion always holds; it's
                    // what lets `backend`'s own main-actor-isolated method be
                    // called from a plain, unisolated closure type.
                    runNotifier.activate(streamFactory: { [backend] in
                        MainActor.assumeIsolated { backend.makeFirehoseStream() }
                    })
                }
        }
        .defaultSize(width: 1440, height: 900)

        Settings {
            SettingsView(model: model, backend: backend, notifier: runNotifier)
                .tint(Color.rupuBrand)
        }
    }

    private var preferredColorScheme: ColorScheme? {
        switch appearance {
        case "light": .light
        case "dark": .dark
        default: nil
        }
    }
}
