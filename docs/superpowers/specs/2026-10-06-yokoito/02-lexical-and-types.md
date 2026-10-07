# 02. Lexical structure and types

## 2.1 Source text

- **Encoding.** Files are UTF-8, use the extension `.yoko`, and have no byte-order mark.
- **Whitespace is insignificant** except for newlines, which end statements (§2.6). Indentation is a formatter convention, not syntax.
- **Case.** Keywords are lower-case and reserved (§2.3). Identifiers are case-sensitive.

## 2.2 Comments

```
// line comment
/* block comment — may span lines, does not nest */
/// doc comment — attaches to the next machine, flow, type, enum, event,
/// input field, state, or named step. Rendered in hover, graph tooltips, docs.
```

Doc comments may use Markdown. A doc comment that precedes nothing it can attach to triggers lint warning `dangling-doc`.

## 2.3 Keywords

Keywords are **contextual**. A keyword spelling is a keyword only where it can begin a declaration, statement, block clause, transition or modifier. Everywhere a *name* is expected, any keyword spelling is an ordinary identifier:
- option and setting names (`timeout: 7d`, `wait: 30m`, `from: end`, `defaults { timeout 30m }`)
- bare-word option and setting values (`losers: cancel`, `exhausted: continue`)
- field names after `.` (`run.outcome`, `loop.iteration`, `scope.root`)
- record keys
- enum values (`retry`, `continue`)
- branch, stage, state and region names (`history { … }`)
- binding and parameter names (`scope = ask …`, `round`)
- names in expression position (`input`, `event`, `output`, `budget`, `goals`)

Only the **hard keywords** are never identifiers: `true false null and or not in is`.

**Statement starts.** At the start of a statement, a word followed by `=` (not `==`) is always a binding or assignment, never a keyword. So `run = …` binds a value named `run`, while `run `cmd`` is a run step.

**Core keyword spellings** (contextual):

```
machine uses import as schema enum type event input output trigger manual cron every
on if else with instance per keyed route var let defaults limits start failure finally flow
state states initial final parallel region history deep entry exit do done after
keep_waiting reenter test given mock expect advance returns called match fork map in
pipeline stage race async await all any detach worklist push loop while for carry break
continue yield goto end cancel raise fail assert note wait sleep until emit send call
try catch saga compensate within throttle lock semaphore budget place timeout retry
cache by on_error migrate from version restart
```

Extensions add contextual keyword spellings (§9.2). The `agentic` extension adds `agent tool run approve ask best_of pursue round goals scope pool`. They are keywords only in files that `use` the extension, and even there only in keyword positions.

## 2.4 Names

| Kind | Form | Examples | Used for |
|---|---|---|---|
| value name | `[a-z_][a-z0-9_]*` | `triage`, `max_files`, `_tmp` | bindings, variables, inputs, steps, states, flows, fork branches |
| type name | `[A-Z][A-Za-z0-9]*` | `Finding`, `Severity`, `Triage` | types, enums, record types |
| agent ref | `@[a-z0-9][a-z0-9_-]*` | `@security-triager`, `@patcher` | agents; resolved against the host catalogue |
| dotted name | `name(.name)+` | `github.issue.labeled`, `approval.timeout`, `scm.prs.create` | events, error types, tools, enum-qualified values |
| pattern | dotted name with a trailing `.*` | `provider.*`, `github.issue.*` | event and error matching |

- A dotted name is never an expression. It is only valid where an event, error or tool is expected.
- Inside expressions, `a.b` is field access.
- The parser always knows which of the two it is reading from the position, so there is no ambiguity.

## 2.5 Literals

| Literal | Syntax | Type | Notes |
|---|---|---|---|
| integer | `42`, `1_000_000`, `0x1F` | `int` | 64-bit signed; underscores are ignored |
| float | `3.5`, `1e-3`, `0.8` | `float` | IEEE-754 double |
| boolean | `true`, `false` | `bool` | |
| null | `null` | `null` | only assignable to `T?` |
| duration | `30s`, `5m`, `2h`, `7d`, `1w`, `1h30m`, `250ms` | `duration` | units `ms s m h d w`; components must appear in decreasing order |
| string | `"text {{ expr }}"` | `string` | interpolated (§3.6); escapes `\" \\ \n \t \u{1F600} \{` |
| multi-line string | `"""` … `"""` | `string` | dedented (below); interpolated |
| raw string | `r"…"`, `r"""…"""` | `string` | no escapes, no interpolation |
| command | `` `cargo test {crate} --release` `` | `Command` | argv vector (below) |
| list | `[a, b, c]` | `list<T>` | trailing comma allowed |
| record | `{ file: f, hits: 3 }` | record | keys are value names or `"quoted strings"`; shorthand `{ file }` means `{ file: file }` |

**Interpolation is lexed in expression mode.** Inside `{{ … }}` and `{% … %}`, the lexer switches to expression mode, tracking nesting depth (as Swift and Kotlin do), so expression string literals don't end the enclosing string: `"by {{ names | join(", ") }}"` is one string. yokoito has no single-quoted strings; `'…'` appears only inside command literals.

**Dedent rule for `"""`:**
1. The opening `"""` must end its line, and the closing `"""` must start its own line.
2. The common leading whitespace of the non-blank lines in between is removed.
3. The first and last line breaks are dropped.

So prompts can be indented with the code without that indentation reaching the model.

**Command literals:**
- The text between backticks is split on unquoted whitespace into argv elements. `'single quotes'` group words into one element.
- `{expr}` interpolates the expression's string form as **exactly one** argv element, never split. A `list<string>` expression written as `{expr...}` expands to one element per item.
- There is no shell. Characters like `|`, `;`, `&&`, `$()` and `*` are literal text.
- To use a shell, write `` `sh -c 'cmd | grep x'` ``, so the shell is visible in the source.

## 2.6 Statements and line structure

A newline ends a statement except where the statement plainly continues:

- **Inside an open bracket.** Within `(`, `[` or `{`, newlines are ordinary whitespace. Exception: a `{` that opens a statement block, where newlines separate statements.
- **After a trailing operator or comma.** A line ending in a binary operator, `=`, `->`, `=>` or `,` continues on the next line.
- **Before a method chain.** A line starting with `.` continues a method chain.
- **Before a modifier.** A line starting with a **modifier keyword** (`place timeout retry catch after compensate cache on_error`) attaches that modifier to the preceding step. Exception: **inside a `state` body**, a line starting with `after` is always a delayed transition (§7.3). A `do <step>` activity therefore takes its modifiers on the same line; for multi-line modifiers, use `flow { }`.
- **Before a trigger clause.** A line starting with `if` or `with {` continues a preceding `trigger on …` declaration. (`with lock` and `with semaphore` always start a statement.)
- **Semicolons.** `;` separates two statements on one line.

**Calls vs options: whitespace matters before `(`.**
- A call or postfix parenthesis must follow its callee **with no whitespace**: `take(max_files)`, `issues.get(project: repo)`, `host("worker-1")`.
- A `(` preceded by whitespace in a step or block head opens that step's **options**: `map f in files.take(8) (concurrency: 4)`, `wait for github.pr.merged if event.pr.number == pr.number (timeout: 7d)`.
- Inside an expression, a parenthesis preceded by whitespace is grouping: `a * (b + c)`.

## 2.7 Types

### Primitive types
`bool`, `int`, `float`, `string`, `duration`, `time`, and `null` (the type of `null`).

### Built-in structured types
| Type | Meaning |
|---|---|
| `list<T>` | ordered sequence |
| `map<T>` | string-keyed dictionary |
| `{ a: T, b?: U }` | record; `b?` is an optional field |
| `T?` | optional: shorthand for `T \| null` |
| `A \| B` | union |
| `json` | dynamically typed JSON value, the explicit escape hatch |
| `Agent` | an agent reference; the type of `@name` |
| `Command` | a command literal |
| `Error` | `{ type: string, message: string, step?: string, attempt?: int, cause?: Error, data?: json }` |
| `Handle<T>` | the result of `async`; `await` turns it into `T` |
| `ChildRef` | `{ id: string }`, the result of `call machine … detach` |
| `Run` | `{ stdout: string, stderr: string, exit_code: int, duration: duration, lines?: list<string>, json?: json }` (agentic). `lines` is set under `parse: lines`, `json` under `parse: json`. |
| `Approval<F>` | `{ by: list<string>, at: time, form: F, comment?: string }` (agentic) |

### Declarations
```
/// Finding severity. Declaration order is the ordering: low < … < critical.
enum Severity { low, medium, high, critical }

type Finding {
  severity: Severity
  title:    string
  body?:    string          // optional field
  file?:    string
}

type Findings = list<Finding>          // alias

type Triage {
  actionable: bool
  files:      list<string>
  summary:    string
  ..                                    // open record: unknown fields are allowed (and dropped)
}
```

### Rules
- **Records are closed by default.** Validating a value with unknown fields against a closed record type fails, unless the record ends with `..` (open).
- **Enums** are ordered and comparable (`<`, `>=`). Values are written bare (`high`) when the expected type is known, or qualified (`Severity.high`). In JSON they are strings.
- **Bare words as option values.** Step and block options are typed by their schema (core or extension manifest), and many are enums. A bare word in option or setting position (`losers: cancel`, `workspace: sync`, `exhausted: continue`) resolves against the option's enum by the same expected-type rule. A binding in scope with the same name takes precedence, and the checker warns `option-shadowed`.
- **Record fields** in `type` declarations, `input { }` and record types may be separated by newlines, `;` or `,`.
- **One namespace for flows and bindings.** Flow names and binding names share one namespace per file, so a binding may not reuse a flow's name (`E0105 binding-shadows-flow`).
- **Implicit conversion** happens only from `int` to `float`. Every other conversion uses a function (`string(x)`, `int(s)`, `duration(s)`).
- **Union narrowing.** `match`, `x != null`, `ok(x)` and `x is Type` narrow a union within the guarded scope.
- **Inference.** `let`, step bindings and `var`s with initialisers infer their type. `input` fields, `var`s without initialisers, and `flow` parameters must be annotated.
- **Generics.** There are no user-defined generics. The only parameterised types are the built-ins listed above.
- **Optional bindings.** A step binding is `T?` when the step may not produce a value: it is guarded by `if`, has `on_error continue` or a catch that falls through, or is a race or join loser. The checker requires handling the null before use (`x ?? default`, `x?.f`, `if x != null`).
- **JSON Schema import.** `import schema "triage_v1" as Triage` turns a JSON Schema (draft-07+) into a yokoito type. Unsupported schema features (`patternProperties`, conditional schemas) become `json` for the affected field, with a warning.

### JSON mapping (IR, journals, effect payloads)
| yokoito | JSON |
|---|---|
| `int`, `float`, `bool`, `string`, `null` | native |
| `duration` | string in yokoito syntax: `"1h30m"` |
| `time` | RFC 3339 string: `"2026-10-06T09:00:00Z"` |
| enum | string: `"high"` |
| record, `map<T>` | object |
| `list<T>` | array |
| `Agent` | string: `"@patcher"` |
| `Handle<T>` | an opaque handle id; never persisted outside the journal |

## 2.8 Examples

```
enum Route { page_oncall, queue, ignore }

type Audit {
  file:       string
  suspicious: bool
  pattern?:   string
  severity:   Severity
}

input {
  /// Repository to operate on, as owner/name.
  repo:           string
  max_files:      int      = 8
  severity_floor: Severity = high
  reviewers:      list<Agent> = [@security-reviewer, @maintainability-reviewer]
}
```
