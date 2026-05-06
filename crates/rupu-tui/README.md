# rupu-tui

Live + replay terminal viewer for rupu runs.

See:
- Spec: `../../docs/superpowers/specs/2026-05-05-rupu-slice-c-tui-design.md`
- User docs: `../../docs/tui.md`

## Cross-emulator smoke matrix (manual)

Before tagging a release, run `./target/release/rupu watch <fixture> --replay --pace=20` in each:

- [ ] iTerm2 3.5+ on macOS — glyphs render, colors correct
- [ ] Alacritty 0.13+ on macOS — glyphs render, colors correct
- [ ] Windows Terminal 1.20+ on Windows 11 — glyphs render, colors correct
- [ ] tmux 3.4 in iTerm2 — glyphs render (may need `set -g default-terminal "tmux-256color"`)
- [ ] `NO_COLOR=1 rupu watch <fixture>` — glyphs only, no color
