# 03. Expressions and templates

weft has **one** expression language. It is used in guards (`if`, `until`, `when`), step options and tool arguments, assignments, outputs, and inside prose templates. Expressions are:

- **pure.** No side effects. `now()`, `uuid()` and `random()` are deterministic per macrostep (§3.5).
- **total.** Every expression terminates, because there are no loops or user recursion. Evaluation that goes wrong (division by zero, an out-of-range index) raises a typed `expression.*` error.
- **typed.** They are checked at compile time against declared and inferred types. There is no truthiness.

The semantics follow CEL (Common Expression Language). The surface syntax is weft's own, chosen for readability.

## 3.1 Operators and precedence

From loosest to tightest binding:

| Level | Operators | Associativity | Notes |
|---|---|---|---|
| 1 | `x => expr` | right | lambda; only as an argument to a higher-order function |
| 2 | `c ? a : b` | right | `c` must be `bool` |
| 3 | `??` | left | `a ?? b`: `a` if not null, else `b`; `a: T?`, `b: T` gives `T` |
| 4 | `or` | left | short-circuit; `bool` operands |
| 5 | `and` | left | short-circuit; `bool` operands |
| 6 | `not` | prefix | `bool` |
| 7 | `== != < <= > >= in` `is` | none | comparisons don't chain; `in` tests list or map-key membership or substring; `x is Type` narrows |
| 8 | `+ -` | left | numbers; `+` also concatenates `string` and `list`; `time ± duration`; `duration ± duration` |
| 9 | `* / %` | left | numbers; `duration * int`; `/` on ints truncates |
| 10 | unary `-` | prefix | |
| 11 | `.f` `?.f` `[i]` `[a:b]` `(args)` `.method(args)` | left | field, optional field, index, slice, call, method |

**Comparisons:**
- `==` is structural equality on every type.
- Ordering (`<` and friends) is defined on numbers, strings (lexicographic, by code point), durations, times and enums (declaration order).
- Comparing values of different types is a compile error.

**Null handling.**
- `x?.f` gives `null` if `x` is null, and **short-circuits the rest of the postfix chain**: `audit?.vulns.size()` is `null` (typed `int?`) when `audit` is null, and `.vulns.size()` is checked against the non-null type.
- `x.f` on a `T?` is a compile error.
- Indexing past the end, `m["missing"]` on a map, and `int("abc")` raise `expression.index`, `expression.key` and `expression.convert` respectively. Use `.get(k)` / `.at(i)` for the `T?`-returning forms.

## 3.2 Names in scope

| Name | Where | Type |
|---|---|---|
| `input.<f>` or bare `<f>` | everywhere | the declared input field. Bare input names are in scope unless shadowed. |
| step bindings | after the step, within its scope | the step's result type |
| `var`s | their declaring scope | declared type |
| `event` | `trigger … if/with`, `on <event>` handlers and transitions, `during … on` handlers, the guard of `wait for … if` | the payload type of the event being matched (in a `wait for` guard, the *candidate* event). **Not** in scope in flow bodies; map the starting event into `input` with the trigger's `with { }`. |
| `entity` | instance machines (`instance per …`) | the entity record (issue/PR), from the host |
| `run` | everywhere | `{ id, machine, version, status, started_at, outcome? }` |
| `output` | `on done` transitions; `catch` fall-through | the activity's or step's result |
| `error` / `e` | `catch … as e`, `on failure as e` | `Error` |
| `loop.iteration`, `prev.<name>` | inside `loop` / `while` | `int`; previous pass's binding `T?` |
| `index`, `count`, `item key` | inside `map`, `pipeline`, `worklist` | `int`, `int`, `string` |
| `results` | the `until:` option of `map` / `worklist` / `pipeline` | the item results completed so far (`list<T?>`) |
| `round` | inside `pursue` | `int` |
| `budget` | inside a `budget` block (core, §6.14) and `pursue` | `{ spent: map<float>, fraction: float, stage: ok\|soft\|hard }` |
| `goals`, `coverage`, `steering` | agentic (ch. 09), inside `pursue` | host-provided, journaled bindings (`steering` is a var) |

## 3.3 Lambdas and higher-order functions

```
findings.filter(f => f.severity >= high)
files.map(p => { path: p, ext: p.split(".").last() })
reviews.all(r => r.approved)
audits.sort_by(a => a.severity).reverse().take(5)
findings.group_by(f => f.file)            // map<list<Finding>>
```

A lambda's parameter type is inferred from the receiver. Lambdas cannot be stored in variables or returned. They exist only as arguments, which keeps expressions total.

## 3.4 Standard library

Methods are written `receiver.method(args)`. Free functions are written `name(args)`.

**Lists** (`list<T>`):
- shape: `size() int` · `is_empty() bool` · `first() T?` · `last() T?` · `at(i) T?`
- searching: `contains(x) bool` · `index_of(x) int?` · `find(f) T?` · `any(f) bool` · `all(f) bool` · `count(f) int`
- transforming: `map(f)` · `filter(f)` · `flat_map(f)` · `flatten()` (on `list<list<T>>`) · `reverse()` · `distinct()` · `distinct_by(f)`
- slicing: `take(n)` · `skip(n)` · `chunks(n) list<list<T>>`
- ordering: `sort()` · `sort_by(f)`
- grouping and aggregation: `group_by(f) map<list<T>>` · `zip(other)` · `enumerate() list<{index, value}>` · `sum()` · `min()` · `max()` · `min_by(f)` · `max_by(f)` · `join(sep) string` (on `list<string>`)

**Maps** (`map<T>`): `size()` · `keys()` · `values()` · `entries() list<{key, value}>` · `get(k) T?` · `has(k)` · `merge(other)` · `map_values(f)` · `filter(f)` (on entries)

**Strings:**
- `size()` · `lower()` · `upper()` · `trim()`
- `starts_with(s)` · `ends_with(s)` · `contains(s)`
- `split(sep)` · `replace(a, b)`
- regex: `matches(re) bool` · `find_all(re) list<string>` · `capture(re) map<string>?`
- `lines() list<string>` · `truncate(n) string`

**Records:** `with(field: value, …)` returns a copy with fields replaced. `fields()` gives `list<string>`.

**Conversion and construction:**
- `string(x)` · `int(x)` · `float(x)` · `bool(s)`
- `duration(s)` · `time(s)` · `json(s) json` (parse) · `tojson(x) string`
- `range(n) list<int>` · `range(a, b)`

**Time:**
- `now() time`
- `next(spec) time`, where `spec` is like `"mon 09:00"`, `"09:00"` or a cron expression; evaluated in the machine's time zone
- `time.date()` · `time.weekday()` · `time + 2h` · `t1 - t2 duration`

**Identity and randomness:** `uuid() string` · `random() float` · `random_int(a, b) int` · `shuffle(list)`. All are deterministic per run (§3.5).

**Step metadata.** These take a step binding or block result:
- `ok(x) bool` · `failed(x) bool` · `skipped(x) bool`
- `status(x)`, one of `succeeded \| failed \| skipped \| cancelled`
- `recovered(x) bool`: true when the step failed but a `catch` (or an interrupting `after` body) fell through and supplied its value
- `error(x) Error?` · `attempts(x) int` · `elapsed(x) duration` · `started_at(x) time?`
- `outcome(x) string?`: for `call machine` results, the child's outcome name (`completed`, `rejected`, …)

**Block metadata.** These take a block result:
- `branches(x)`: per-branch statuses of a `fork`/`race`
- `unprocessed(x)` and `stopped_by(x)`: for a `worklist`
- `stages(x)`: per-item stage yields of a `pipeline`

**State layer:** `in_state(path) bool`, e.g. `in_state("monitoring.ci.green")`. It enables milestone patterns (Appendix B, WCP-18).

**Typing:** `type_of(x) string` (useful on `json`) and `x is Type`.

Extensions add functions under their own names (ch. 09).

## 3.5 Determinism

- `now()` is the `at` timestamp of the input that started the current macrostep. Within one macrostep, `now()` is constant.
- `uuid()`, `random*()` and `shuffle()` draw from a pseudo-random generator seeded per instance. Its state is in the snapshot.
- So replaying an instance's journal gives bit-identical values without journaling them separately.

**Evaluation budget.** One expression evaluation is capped at 1,000,000 elementary operations; going over raises `expression.budget`. This cap is a guard against pathological inputs, such as a `flat_map` over a huge list, not a normal limit.

## 3.6 Prose templates

Every string literal except raw strings is a template.

### Interpolation
```
"Audit {{ file }} ({{ index + 1 }}/{{ count }})"
"{{ findings | tojson }}"
"{{ prev.verify?.stderr ?? "(first pass)" | truncate(4000) }}"
```
- `{{ expr }}` evaluates an expression with the same language and scope as anywhere else, then renders it:
  - strings verbatim
  - numbers and booleans in canonical form
  - durations and times in weft and RFC 3339 syntax
  - `null` as the empty string
  - records and lists as compact JSON
- `\{{` writes a literal `{{`.

### Filters (`expr | filter(args)`)
| Filter | Effect |
|---|---|
| `tojson` / `tojson(pretty: true)` | JSON encoding |
| `join(sep)` | joins a list into a string |
| `indent(n)` | indents every line after the first |
| `truncate(n)` / `truncate(n, from: end)` | cuts to `n` characters and adds a marker |
| `lines` | splits into lines (for `{% for %}`) |
| `upper`, `lower`, `trim` | string case and trimming |
| `default(v)` | same as `?? v` |
| `bullet` | renders a list as `- item` lines |
| `code(lang)` | wraps in a fenced code block |

Filters are a readability layer: `a | f(x)` is exactly `f(a, x)` from the standard library.

### Control blocks
```
"""
{% if blocking.is_empty() %}No blocking findings.{% else %}
Fix these {{ blocking.size() }} findings:
{% for f in blocking %}- [{{ f.severity }}] {{ f.title }}{% if f.file != null %} ({{ f.file }}){% end %}
{% end %}{% end %}
"""
```
- `{% if cond %}…{% else if cond %}…{% else %}…{% end %}`
- `{% for x in list %}…{% end %}`, with `loop.index` available inside
- `{%-` and `-%}` trim the whitespace before or after the tag.

### Strictness
Templates are checked like code:
- An unknown name, a field access on `T?` without `?.`, or a non-bool `if` condition is a compile error.
- Interpolating a `json`-typed or unbounded list value without `truncate` raises lint `prompt-unbounded-value` (a warning).

## 3.7 Examples

```
let blocking = review.findings.filter(f => f.severity >= severity_floor)
let label    = blocking.is_empty() ? "clean" : "needs-fix"
let by_file  = blocking.group_by(f => f.file ?? "(unknown)")
let first_critical = audits.find(a => a.severity == critical)
let summary  = "{{ blocking.size() }} blocking in {{ by_file.size() }} files"
let due      = now() + 72h
let is_weekend = now().weekday() in ["sat", "sun"]
```
