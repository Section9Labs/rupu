# Runtime ownership inside the signed desktop app

Build the bundled runtime with **compile-time** `RUPU_INSTALL_METHOD=desktop`.
The CLI's `option_env!` embeds that ownership into the executable; it remains
app-owned when copied elsewhere or launched with a different environment.
Setting a runtime environment variable is not a substitute for the build stamp.

The desktop-owned runtime refuses `rupu update`, `rupu update --rollback`, and
the internal `__apply-update` installer entry point before downloading or
replacing a binary. The hidden installer also refuses before reading staged
content. The user updates `rupu.app` instead, preserving the outer app signature
and the app's pinned backend compatibility.

`rupu update --check` alone remains read-only. Both its available-update message
and the passive update notice point to updating the app. Combining `--check`
with `--rollback` still refuses the write operation. Ordinary CLI and existing
package-manager installations retain their previous ownership rules.

The application signs the runtime as nested executable code before signing the
outer bundle. Its manifest records the exact source, input binary, signed binary,
compiler, resource, and license provenance; a desktop stamp is an ownership
control, not a release or notarization claim.
