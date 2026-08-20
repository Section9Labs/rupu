import AppKit
import SwiftUI
import RupuShell
import RupuStore
import RupuDesign
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
final class AppDelegate: NSObject, NSApplicationDelegate {
    private static let logger = Logger(subsystem: "com.section9labs.rupu", category: "lifecycle")

    var backend: BackendController?
    private var sigtermSource: DispatchSourceSignal?

    func applicationDidFinishLaunching(_ notification: Notification) {
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

@main
struct RupuApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @State private var model = AppModel()
    @State private var backend = BackendController()
    @AppStorage("appearance") private var appearance: String = "system"

    var body: some Scene {
        WindowGroup {
            RootView(model: model, backend: backend)
                .frame(minWidth: 1150, minHeight: 760)
                .preferredColorScheme(preferredColorScheme)
                .tint(Color.rupuBrand)
                .onAppear { appDelegate.backend = backend }
        }
        .defaultSize(width: 1440, height: 900)

        Settings {
            SettingsView()
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
