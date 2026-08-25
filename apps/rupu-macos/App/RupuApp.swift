import AppKit
import SwiftUI
import RupuShell
import RupuStore
import RupuDesign
import RupuMenuBar
import RupuSituation
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
    ///
    /// The `didSet` drains `pendingNotificationRunID` (final-review fix —
    /// M1): a notification tap that LAUNCHES the app delivers
    /// `didReceive` before `RootView` has ever appeared, so `model` is
    /// still `nil` at routing time and the deep link used to be dropped on
    /// the floor — the app came forward on the default route with no
    /// indication the tap had meant anything. Parking the run id and
    /// replaying it the moment `model` is attached turns the launch path
    /// into the same navigation the already-running path gets.
    var model: AppModel? {
        didSet { drainPendingNotificationRoute() }
    }
    /// The run id from a notification tap that arrived before `model`
    /// existed — see `model`'s doc comment. Only ever the MOST RECENT such
    /// tap: they all route to the same window, and landing on the last one
    /// tapped is the least surprising outcome.
    private var pendingNotificationRunID: String?
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
            if let model {
                model.navigate(to: .runDetail(id: runID, host: nil))
            } else {
                // Cold-launch tap: `RootView.onAppear` hasn't run yet, so
                // there is nothing to navigate. Park it — `model`'s `didSet`
                // replays it (final-review fix — M1).
                pendingNotificationRunID = runID
            }
        }
        NSApp.activate(ignoringOtherApps: true)
        frontMainWindow()
    }

    /// Replays a tap that arrived before `model` was attached. Runs from
    /// `model`'s `didSet`, i.e. from `RootView.onAppear` — the same point
    /// in the launch sequence at which any other deep link would be
    /// honored.
    private func drainPendingNotificationRoute() {
        guard let model, let runID = pendingNotificationRunID else { return }
        pendingNotificationRunID = nil
        model.navigate(to: .runDetail(id: runID, host: nil))
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
        // Live-validation fix: the old second lookup ("any window that isn't
        // Settings") matched the MenuBarExtra POPOVER — itself a borderless
        // NSWindow in `NSApp.windows` — so in the windowless state this
        // fronted the popover and the `openMainWindow` fallback never ran.
        // Match the main scene deliberately instead: SwiftUI stamps
        // `WindowGroup(id: "main")` windows with the scene id verbatim or a
        // derived "main-AppWindow-N" form (observed both across macOS
        // versions), and as a last resort any regular TITLED window that
        // isn't the Settings scene — the popover is borderless and can never
        // satisfy `.titled`.
        let isMainSceneWindow: (NSWindow) -> Bool = { window in
            if let id = window.identifier?.rawValue {
                if id == RupuApp.mainWindowID || id.hasPrefix("\(RupuApp.mainWindowID)-") { return true }
                // Phase 6B, Task 7 fix: the Situation Room window
                // (`Window(id: "situation")`) is a real, TITLED NSWindow
                // (fullscreen or not — `.styleMask.contains(.titled)` stays
                // true even once it's entered fullscreen), so without this
                // exclusion the fallback below would happily front it for a
                // notification tap / "Open rupu" / a needs-you deep-link
                // instead of the actual main content window — the exact
                // same class of bug the Settings exclusion right below
                // already fixes for that scene.
                if id == RupuApp.settingsWindowID || id == RupuApp.situationWindowID
                    || id.hasPrefix("\(RupuApp.situationWindowID)-") { return false }
            }
            return window.styleMask.contains(.titled)
        }
        if let target = NSApp.windows.first(where: isMainSceneWindow) {
            target.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
        } else {
            openMainWindow?()
            NSApp.activate(ignoringOtherApps: true)
        }
    }

    /// Enters fullscreen for the Situation Room window the moment it
    /// exists — called from `SituationRoomScreen`'s own `.onAppear` (via
    /// `RupuApp`'s `Window(id: "situation")` scene) rather than something
    /// this delegate initiates on its own. Idempotent: a window already in
    /// `.fullScreen` is left alone (re-toggling would EXIT fullscreen,
    /// which is the opposite of what a re-appearing/re-triggered call
    /// should do).
    ///
    /// Believed correct but, like `frontMainWindow()`'s windowless
    /// fallback, this is a GUI-check item for the controller's live
    /// validation pass — there's no reliable way to assert real
    /// `NSWindow`/`toggleFullScreen` behavior under `swift test` (no real
    /// app bundle/run loop).
    func enterSituationRoomFullScreen(attempt: Int = 0) {
        // Live-validation fix: at `.onAppear` time SwiftUI has not yet
        // stamped the scene's identifier onto its `NSWindow`, so the lookup
        // found nothing and the old single-shot guard silently no-oped —
        // the SR opened as a plain window (observed live). Retry on the
        // main queue, bounded: the identifier lands within the first few
        // run-loop turns; ten 100ms attempts is far past that, and giving
        // up leaves an entirely usable plain window rather than looping.
        guard let window = NSApp.windows.first(where: { window in
            guard let id = window.identifier?.rawValue else { return false }
            return id == RupuApp.situationWindowID || id.hasPrefix("\(RupuApp.situationWindowID)-")
        }) else {
            if attempt < 10 {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { [weak self] in
                    self?.enterSituationRoomFullScreen(attempt: attempt + 1)
                }
            }
            return
        }
        guard !window.styleMask.contains(.fullScreen) else { return }
        window.toggleFullScreen(nil)
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
    /// Identity of the `CPClient` `runNotifier`/`menuBarStore` are currently
    /// bound to — the same `backend.clientIdentity()` seam every
    /// store-owning screen uses to notice a client swap (see that method's
    /// doc comment). Final-review fix (I1): without this, `RunNotifier` was
    /// bound for the app's whole lifetime to whatever backend existed at
    /// its FIRST activation. Its `activate(streamFactory:)` no-ops while
    /// `task != nil`, and the running loop only re-enters `streamFactory()`
    /// when the current stream's `events()` sequence ENDS — which
    /// `JSONEventStream` never lets happen, since it reconnects internally,
    /// forever, to its construction-time URL and token. So an
    /// embedded→remote switch, a remote reconnect to a different CP, or a
    /// token change left notifications wired to the abandoned backend with
    /// nothing to notice. Comparing identity and forcing a
    /// `deactivate()` + `activate()` is what actually re-enters the factory.
    @State private var boundClientIdentity: ObjectIdentifier?
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
    /// Phase 6B, Task 7: the Situation Room's own `Window(id:)` identifier —
    /// see `frontMainWindow()`'s doc comment for why this must be excluded
    /// from its titled-window fallback, same as `settingsWindowID`.
    static let situationWindowID = "situation"

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
        // Phase 6B, Task 7: "Enter Situation Room" — `CommandGroup(after:
        // .toolbar)` is the placement SwiftUI folds into the View menu
        // (the same section View-menu items like "Show Toolbar" land in),
        // which is where an "enter a special full-window mode" command
        // belongs. Attached to this scene (not the `Window(id: "situation")`
        // scene below) so the item exists in the menu bar whether or not
        // Situation Room is currently open — `openWindow(id:)` creates the
        // window on demand either way.
        .commands {
            CommandGroup(after: .toolbar) {
                Button("Enter Situation Room") {
                    openWindow(id: RupuApp.situationWindowID)
                }
            }
        }

        Settings {
            SettingsView(model: model, backend: backend, notifier: runNotifier)
                .tint(Color.rupuBrand)
        }

        // Phase 6B, Task 7: the Situation Room — a fullscreen live wall,
        // opened via "Enter Situation Room" (above) or the Dock/Mission
        // Control. Dark always (`.preferredColorScheme(.dark)` — a
        // deliberate exception to `appearance`; see `SituationRoomScreen`'s
        // doc comment) and enters fullscreen the moment its window exists
        // (`AppDelegate.enterSituationRoomFullScreen()`, called from
        // `.onAppear` since SwiftUI has no declarative "open already
        // fullscreen" scene modifier).
        Window("Situation Room", id: RupuApp.situationWindowID) {
            SituationRoomScreen(model: model, backend: backend, frontMainWindow: { appDelegate.frontMainWindow() })
                // `minWidth` review fix round 1, ruling 11: 900 let a
                // non-fullscreen window shrink narrower than `PulseStrip`'s
                // own intrinsic width (brand cell + six KPI tiles, ~1050pt),
                // clipping the instrument strip. 1060 gives a small margin
                // above that estimate.
                .frame(minWidth: 1060, minHeight: 600)
                .preferredColorScheme(.dark)
                .tint(Color.rupuBrand)
                .onAppear {
                    appDelegate.enterSituationRoomFullScreen()
                }
        }
        .defaultSize(width: 1440, height: 900)

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
            // **The activation observers live on the LABEL, deliberately**
            // (final-review fix — I1). `runNotifier`/`menuBarStore` are
            // app-lifetime subscribers that must notice a backend identity
            // change no matter what the operator has open, and every other
            // candidate host is conditionally alive: the `WindowGroup`'s
            // observers below die with the main window (routinely closed —
            // this app keeps running windowless), and the popover CONTENT
            // above only exists while the menu is actually open. The
            // `MenuBarExtra` label is the one view in this scene graph that
            // is mounted for the entire life of the process, so it is the
            // only place these observers can be attached and still fire
            // during the exact states they exist to cover. The
            // `WindowGroup`'s calls stay as they are — every activation
            // path here is idempotent, so a duplicate is a no-op.
            MenuBarStatusLabel(hasAttention: (menuBarStore.counts?.awaitingApproval ?? 0) > 0)
                .onChange(of: backend.health) { _, newHealth in
                    guard case .healthy = newHealth else { return }
                    activateHealthDependents()
                }
                // Reading `clientIdentity()` in the label's body registers
                // `BackendController`'s client as an observed dependency, so
                // this fires on the swap itself — not only when the swap
                // happens to coincide with a health transition (a remote
                // reconnect that stays healthy throughout produces no
                // health change at all).
                .onChange(of: backend.clientIdentity()) { _, _ in
                    activateHealthDependents()
                }
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
    ///
    /// **Rebinds on a client-identity change** (final-review fix — I1): see
    /// `boundClientIdentity`'s doc comment for why `RunNotifier` needs an
    /// explicit `deactivate()` before the re-`activate()` — its running
    /// loop otherwise never re-enters `streamFactory()` and stays wired to
    /// the abandoned backend forever. The factory handed to `activate` is
    /// freshly built each time, so the new loop's first
    /// `backend.makeFirehoseStream()` reads the CURRENT `activeConfig`
    /// (URL + token), not the one captured at first launch.
    /// `menuBarStore.activate(client:)` needs no such teardown — it takes
    /// the client by parameter and its poll loop reads whatever `client`
    /// currently holds on every tick.
    private func activateHealthDependents() {
        let identity = backend.clientIdentity()
        if identity != boundClientIdentity {
            boundClientIdentity = identity
            runNotifier.deactivate()
        }
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
