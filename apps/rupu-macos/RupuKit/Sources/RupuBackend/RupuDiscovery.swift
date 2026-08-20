import Foundation

/// Locates the `rupu` CLI binary on disk.
///
/// Resolution order: an explicit override path (if executable) → `which
/// rupu` run through the user's login shell (so a Homebrew/asdf/nvm-style
/// `PATH` set up in `.zshrc` is honored) → a fixed list of common install
/// locations, first executable wins.
public struct RupuDiscovery {
    /// Common install locations, checked in order when `which` comes up
    /// empty.
    public static let defaultPaths: [String] = [
        "/opt/homebrew/bin/rupu",
        "/usr/local/bin/rupu",
        "~/.local/bin/rupu",
    ]

    public static func find(
        override: String?,
        searchPaths: [String] = defaultPaths,
        which: (String) -> String? = loginShellWhich
    ) -> String? {
        if let override, isExecutable(override) {
            return override
        }
        if let found = which("rupu"), isExecutable(found) {
            return found
        }
        for path in searchPaths {
            let expanded = expandTilde(path)
            if isExecutable(expanded) {
                return expanded
            }
        }
        return nil
    }

    /// Runs `which <name>` through the user's login shell so shell-profile
    /// `PATH` customizations (Homebrew, asdf, nvm, ...) are honored the
    /// same way they would be from a Terminal window.
    public static func loginShellWhich(_ name: String) -> String? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/zsh")
        process.arguments = ["-lc", "which \(name)"]

        let stdout = Pipe()
        process.standardOutput = stdout
        process.standardError = Pipe()

        do {
            try process.run()
        } catch {
            return nil
        }
        process.waitUntilExit()

        guard process.terminationStatus == 0 else { return nil }
        let data = stdout.fileHandleForReading.readDataToEndOfFile()
        guard let output = String(data: data, encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines), !output.isEmpty
        else {
            return nil
        }
        return output
    }

    private static func isExecutable(_ path: String) -> Bool {
        FileManager.default.isExecutableFile(atPath: path)
    }

    private static func expandTilde(_ path: String) -> String {
        (path as NSString).expandingTildeInPath
    }
}
