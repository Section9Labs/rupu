import AppKit
import SwiftUI
import RupuShell
import RupuStore
import RupuDesign
import RupuMenuBar
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
    /// Review fix (round 1): `frontMainWindow()` below could previously only
    /// front an ALREADY-EXISTING window — with the main window closed (the
    /// routine state for a menu-bar-capable app; closing the last window
    /// does not quit this app), "Open rupu", every needs-you row's
    /// deep-link, and "New run" all silently did nothing. AppKit has no way
    /// to create a SwiftUI `WindowGroup`'s window on its own — only
    /// SwiftUI's `@Environment(\.openWindow)` action can — so `RupuApp`'s
    /// `.onAppear` hands this closure in (`{ openWindow(id: RupuApp.
    /// mainWindowID) }`), and `frontMainWindow()` falls back to calling it
    /// when there is no existing window to front.
    var openMainWindow: (() -> Void)?
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
    /// or the menu-bar status window (Task 8's `MenuBarExtra` popover). A
    /// notification tap must always land on the app's real content, so this
    /// picks deliberately: prefer the window carrying `RupuApp`'s own
    /// explicit `WindowGroup(id: "main")` identifier, and if that's ever
    /// unavailable for some reason, fall back to any window that ISN'T the
    /// Settings scene's window (`"com_apple_SwiftUI_Settings_window"` is
    /// SwiftUI's own stable internal identifier for a macOS `Settings { }`
    /// scene's window) — the one hard requirement either way is that the
    /// Settings window itself is never the one fronted.
    ///
    /// Non-`private` (Task 8): `RupuApp`'s `MenuBarExtra` scene reuses this
    /// exact mechanism for "Open rupu", every needs-you row's deep-link, and
    /// "New run", via `appDelegate.frontMainWindow()` — same target-window
    /// resolution a notification tap already relies on, not a second copy of
    /// it.
    ///
    /// **Windowless fallback** (review fix, round 1): when NO window
    /// matches either lookup (the main window was closed and nothing else
    /// is up but Settings), this now invokes `openMainWindow` — SwiftUI's
    /// `@Environment(\.openWindow)` action, captured by `RupuApp` and handed
    /// in here — to actually CREATE the main `WindowGroup`'s window, then
    /// activates the app so it comes forward. Every caller of
    /// `frontMainWindow()` (a notification tap, "Open rupu", a needs-you row
    /// deep-link, "New run") gets this fallback for free just by routing
    /// through here, which was the point of centralizing this lookup in the
    /// first place.
    ///
    /// `openWindow(id:)` has no completion callback — there is no signal
    /// this method can wait on for "the window now exists and is ready to
    /// host a sheet/receive a route". Every caller that needs to act
    /// afterward (most notably `MenuBarView`'s "New run", which sets
    /// `model.showLauncher = true` right after calling this) relies on
    /// SwiftUI reading `RootView`'s `.sheet(isPresented:)` binding's CURRENT
    /// value at the moment that view mounts — the same mechanism the
    /// onboarding sheet already depends on for a value set before `RootView`
    /// existed at all. This is believed correct but is, deliberately, a
    /// GUI-check item for the controller's live validation pass, not
    /// something this fix claims to have proven from a unit test — there is
    /// no reliable way to unit-test actual `NSWindow` creation/SwiftUI scene
    /// mounting under `swift test` (no real app bundle/run loop).
    func frontMainWindow() {
        let target = NSApp.windows.first(where: { $0.identifier?.rawValue == RupuApp.mainWindowID })
            ?? NSApp.windows.first(where: { $0.identifier?.rawValue != RupuApp.settingsWindowID })
        if let target {
            target.makeKeyAndOrderFront(nil)
        } else {
            openMainWindow?()
            NSApp.activate(ignoringOtherApps: true)
        }
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
    /// Task 8: the `MenuBarExtra` popover's data source. App-level, activated
    /// from the SAME `.onChange(of: backend.health)` seam as `runNotifier`
    /// just above (not from the `MenuBarExtra` scene's own appear/disappear)
    /// — see `MenuBarStore`'s own doc comment for why the attention dot
    /// needs live data even while the popover is closed.
    @State private var menuBarStore = MenuBarStore()
    @AppStorage("appearance") private var appearance: String = "system"
    /// SwiftUI's window-creation action — the only way to actually create a
    /// `WindowGroup`'s window from outside SwiftUI's own view hierarchy.
    /// Handed to `AppDelegate.openMainWindow` in `.onAppear` below so
    /// `AppDelegate.frontMainWindow()`'s windowless fallback (review fix,
    /// round 1) can use it. `@Environment` reads on an `App`-conforming type
    /// work the same way they do on a `View`/`Scene` — this isn't a special
    /// case.
    @Environment(\.openWindow) private var openWindow

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
                    appDelegate.openMainWindow = { openWindow(id: RupuApp.mainWindowID) }
                    // Review fix (round 1): same reappearance/race fallback
                    // `RootView`'s own `hostsFooter.activate` `.onAppear`
                    // guard uses (see that modifier's doc comment) —
                    // `.onChange(of: backend.health)` below only fires on a
                    // POST-installation transition. If a client is already
                    // configured by the time THIS modifier attaches (a fast
                    // local embedded-server health check racing SwiftUI's
                    // own environment/scene setup), the change never fires
                    // and `runNotifier`/`menuBarStore` would sit inert —
                    // `menuBarStore` stuck showing `—` — until the next
                    // health flap, which may never come. Both `activate`
                    // calls are idempotent, so calling them here too is a
                    // no-op whenever `.onChange` already ran normally.
                    activateHealthDependents()
                }
                .onChange(of: backend.health) { _, newHealth in
                    guard case .healthy = newHealth else { return }
                    activateHealthDependents()
                }
        }
        .defaultSize(width: 1440, height: 900)

        Settings {
            SettingsView(model: model, backend: backend, notifier: runNotifier)
                .tint(Color.rupuBrand)
        }

        // Task 8: the menu-bar extra. `.window` style (not `.menu`) so
        // `MenuBarView`'s stat tiles / needs-you list / inline gate actions
        // render as a real SwiftUI view popover rather than being forced
        // into `NSMenuItem` rows. The label is the app's own wordmark with
        // an attention dot (see `MenuBarStatusLabel`'s doc comment) —
        // re-evaluated automatically whenever `menuBarStore`'s `@Observable`
        // `counts` changes, no manual refresh wiring needed.
        MenuBarExtra {
            MenuBarView(store: menuBarStore, model: model, backend: backend, openMainWindow: openMainWindow)
        } label: {
            MenuBarStatusLabel(hasAttention: (menuBarStore.counts?.awaitingApproval ?? 0) > 0)
        }
        .menuBarExtraStyle(.window)
    }

    /// "Open rupu" / a needs-you row's deep-link, from the menu bar: bring
    /// the app forward and front the real content window — same two-step
    /// `NSApp.activate` + `frontMainWindow()` sequence
    /// `AppDelegate.routeNotificationTap` already uses for a notification
    /// tap, reused via `appDelegate.frontMainWindow()` rather than a second
    /// copy of that window-resolution logic.
    private func openMainWindow() {
        NSApp.activate(ignoringOtherApps: true)
        appDelegate.frontMainWindow()
    }

    /// Shared body for both `runNotifier`/`menuBarStore` activation call
    /// sites (`.onAppear`'s fallback and `.onChange(of: backend.health)`'s
    /// normal path — review fix, round 1: factored out so the two stay in
    /// sync rather than risking drift between two copies of the same
    /// activation logic).
    ///
    /// `runNotifier.activate(streamFactory:)` is called unconditionally,
    /// not gated on `backend.client()` — see that method's own doc comment:
    /// it backs off and retries on its own if `streamFactory()` returns
    /// `nil` (backend not configured yet), so calling it before a client
    /// exists is safe and is exactly what lets the `.onAppear` fallback
    /// self-heal without this method needing to duplicate that gating.
    /// `menuBarStore.activate(client:)` DOES need an actual `CPClient` (its
    /// signature takes one directly, unlike `RunNotifier`'s lazy factory
    /// seam), so it's gated the same way `RootView`'s own
    /// `hostsFooter.activate(client:)` `.onAppear` fallback is.
    private func activateHealthDependents() {
        // `MainActor.assumeIsolated` here matches `RunDetailStore.
        // makeRunSignalsFactory`/`OverviewScreen.makeSignalsFactory`'s own
        // bridging into `backend.make*Stream` from inside a factory
        // closure — `streamFactory` is invoked from `RunNotifier.activate`'s
        // task, which always runs on the main actor (`RunNotifier` is
        // `@MainActor`), so the assertion always holds; it's what lets
        // `backend`'s own main-actor-isolated method be called from a
        // plain, unisolated closure type.
        runNotifier.activate(streamFactory: { [backend] in
            MainActor.assumeIsolated { backend.makeFirehoseStream() }
        })
        // `MenuBarStore.activate(client:)` is idempotent (same idiom as
        // `HostsFooterStore`) — a `client` swap on a later healthy
        // transition (embedded/remote mode switch, a manual reconnect) just
        // updates which client the next poll tick uses, it never spawns a
        // second loop.
        if let client = backend.client() {
            menuBarStore.activate(client: client)
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
