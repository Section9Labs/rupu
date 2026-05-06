# rupu-tui test fixtures

`run_smoke/` is a minimal 3-event scripted run used by `make tui-smoke`.

The smoke target exists to catch link / startup regressions in
`./target/release/rupu watch ... --replay`. It does NOT verify
end-to-end correctness — those checks live in unit + snapshot tests.

The smoke target may exit with "run not found" because `rupu watch`
looks for runs under `~/.rupu/runs/<run_id>/` rather than this fixture
path. That's acceptable — a non-panic exit is what we're checking.
