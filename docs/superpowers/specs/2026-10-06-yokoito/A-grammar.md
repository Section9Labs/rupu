# Appendix A. Grammar (EBNF)

**Notation:**
- `{ x }` means zero or more; `[ x ]` means optional; `|` means alternative.
- `"kw"` is a terminal.
- `sep` is a newline or `;` (§2.6).
- **`⟦ x ⟧`** is a separated sequence: `{ sep } [ x { sep { sep } x } ] { sep }`. Items need a separator *between* them; leading and trailing separators and blank lines are optional. So `{ note "…" }` and `limits { wall_clock 90d }` are valid one-line blocks.
- **`NAME`** is any identifier, *including keyword spellings* (keywords are contextual, §2.3), except the hard keywords `true false null and or not in is`. Wherever this grammar writes `NAME`, a keyword spelling is an ordinary name.
- **Call parens vs option parens:**
  - `"("` written directly after a callee is a **call paren**: no whitespace before it (`take(8)`, `host("w1")`).
  - `OPT_PAREN` is a `(` **preceded by whitespace** in a step or block head, and opens options (§2.6).

Extension keywords parse through `uniform_step` and `uniform_block`. The parser accepts them in uniform shape, and the checker validates them against the extension manifest (§9.2). That is why the grammar never changes per extension.

## A.1 Tokens

```ebnf
IDENT       = ( lower | "_" ) { lower | digit | "_" } ;              (* not a keyword *)
TYPE_NAME   = upper { letter | digit } ;
AGENT_REF   = "@" ( lower | digit ) { lower | digit | "_" | "-" } ;
DOTTED      = IDENT "." IDENT { "." IDENT } ;                         (* positional: events, errors, tools *)
PATTERN     = DOTTED ".*" | IDENT ".*" ;
INT         = digit { digit | "_" } | "0x" hex { hex | "_" } ;
FLOAT       = digit { digit } "." digit { digit } [ exponent ] | digit { digit } exponent ;
DURATION    = duration_part { duration_part } ;                       (* decreasing units *)
duration_part = digit { digit } ( "ms" | "s" | "m" | "h" | "d" | "w" ) ;
STRING      = '"' { char | escape | interpolation } '"'
            | '"""' newline { line } '"""' ;                          (* dedented, §2.5 *)
RAW_STRING  = 'r"' { char } '"' | 'r"""' newline { line } '"""' ;
COMMAND     = "`" { cmd_char | "{" expr [ "..." ] "}" } "`" ;
DOC_COMMENT = "///" { char } newline ;
```

## A.2 Files and items

```ebnf
file            = machine_file | library_file ;
machine_file    = ⟦ import ⟧ machine_header ⟦ machine_item ⟧ ;
library_file    = ⟦ import | decl | flow_def | test_def ⟧ ;

machine_header  = { DOC_COMMENT } "machine" IDENT sep
                  [ "version" STRING sep ]
                  [ "uses" IDENT { "," IDENT } sep ] ;

machine_item    = import | decl | input_decl | output_decl | trigger_decl | instance_decl
                | var_decl | settings_decl | hook | migrate_decl | flow_body | state_decl
                | root_transition | flow_def | test_def | ext_top_block ;

import          = "import" ( STRING | IDENT "/" IDENT | "schema" STRING ) [ "as" ( IDENT | TYPE_NAME ) ] ;
```

## A.3 Declarations and types

```ebnf
decl            = type_decl | enum_decl | event_decl ;
type_decl       = { DOC_COMMENT } "type" TYPE_NAME ( record_type | "=" type ) ;
enum_decl       = { DOC_COMMENT } "enum" TYPE_NAME "{" IDENT { "," IDENT } [ "," ] "}" ;
event_decl      = { DOC_COMMENT } "event" ( DOTTED | IDENT ) [ record_type ] ;

record_type     = "{" [ field_decl { ( sep | "," ) field_decl } ] [ ( sep | "," ) ".." ] [ sep | "," ] "}" ;
field_decl      = { DOC_COMMENT } NAME [ "?" ] ":" type [ "=" expr ] ;

type            = opt_type { "|" opt_type } ;
opt_type        = base_type [ "?" ] ;
base_type       = "bool" | "int" | "float" | "string" | "duration" | "time" | "json" | "null"
                | "Agent" | "Command" | "Error"
                | "list" "<" type ">" | "map" "<" type ">" | "Handle" "<" type ">"
                | TYPE_NAME [ "<" type { "," type } ">" ]
                | record_type | "(" type ")" | literal ;           (* literal types for unions of names *)

input_decl      = "input" record_type ;
output_decl     = "output" expr ":" type ;
var_decl        = "var" IDENT [ ":" type ] [ "=" expr ] ;
```

## A.4 Triggers, instances, settings, hooks, migrations

```ebnf
trigger_decl    = "trigger" ( "manual"
                            | "cron" STRING [ options ]
                            | "every" DURATION [ options ]
                            | "on" event_pattern [ "if" expr ] [ "with" record_expr ] ) ;

instance_decl   = "instance" ( "singleton" | [ "per" IDENT ] "keyed" expr ) [ settings_block ] ;

settings_decl   = ( "defaults" | "limits" ) settings_block ;
settings_block  = "{" ⟦ setting ⟧ "}" ;
setting         = NAME setting_atoms { "," setting_atoms }
                | NAME [ NAME ] settings_block ;                    (* e.g. select { … }, goal x { … } *)
setting_atoms   = setting_atom { setting_atom } ;
setting_atom    = expr | NAME | "in" | modifier_inline ;           (* e.g. lease 4h, renew while active · route github.pr.* by event.pr.ref *)
modifier_inline = "retry" retry_spec | "backoff" backoff | "place" place_spec ;

hook            = "on" "start" block
                | "on" "failure" [ "as" IDENT ] block
                | "on" "cancel" block
                | "finally" block ;

migrate_decl    = "migrate" "from" STRING "{" ⟦ migrate_rule ⟧ "}" ;
migrate_rule    = "state" state_path_pat "->" state_path
                | "var" IDENT ":" type "=" expr
                | "var" IDENT "->" IDENT
                | "drop" "var" IDENT ;
state_path_pat  = IDENT { "." IDENT } [ ".*" ] ;

ext_top_block   = NAME setting_atoms | NAME settings_block ;       (* extension top-level: goals, scope, pool, … *)
```

## A.5 Flows and statements

```ebnf
flow_body       = "flow" block ;
flow_def        = { DOC_COMMENT } "flow" IDENT "(" [ params ] ")" [ options ] [ "->" type ] block ;
                                                                   (* params paren is a call paren; options use OPT_PAREN *)
params          = param { "," param } ;
param           = NAME ":" type [ "=" expr ] ;

block           = "{" ⟦ stmt ⟧ "}" ;
stmt            = let_stmt | var_decl | assign | step | control ;
let_stmt        = "let" IDENT [ ":" type ] "=" expr ;
assign          = lvalue "=" expr ;                                (* lvalue must be a var *)
lvalue          = NAME { "." NAME } ;

step            = [ ( NAME | "_" ) "=" ] step_body { modifier } ;
step_body       = core_leaf | core_block | flow_call | async_step | await_stmt | uniform_step | uniform_block ;

flow_call       = IDENT [ "." IDENT ] "(" [ args ] ")" ;
async_step      = "async" step_body ;
await_stmt      = "await" ( IDENT | ( "all" | "any" ) ( "[" IDENT { "," IDENT } "]" | expr ) ) ;
                                                                   (* expr: list<Handle<T>>; all → list<T>, any → {index, value} *)
```

### A.5.1 Core leaf steps

```ebnf
core_leaf       = wait_step | sleep_step | emit_step | send_step | call_step
                | raise_step | fail_step | assert_step | note_step | detach_step ;

wait_step       = "wait" "for" event_pattern [ "if" expr ] [ options ] ;
sleep_step      = "sleep" ( expr | "until" expr ) ;
emit_step       = "emit" ( DOTTED | IDENT ) [ record_expr ] ;
send_step       = "send" IDENT "[" expr "]" ( DOTTED | IDENT ) [ record_expr ] ;
call_step       = "call" "machine" ( IDENT | "(" expr ")" ) "(" [ args ] ")" [ "detach" ] ;
raise_step      = "raise" ( DOTTED | IDENT ) [ record_expr ] ;
fail_step       = "fail" STRING ;
assert_step     = "assert" expr [ "," STRING ] ;
note_step       = "note" STRING ;
detach_step     = "detach" IDENT ;
```

### A.5.2 Core blocks

```ebnf
core_block      = if_block | match_block | fork_block | map_block | pipeline_block | race_block
                | worklist_block | loop_block | while_block | for_block | with_block | throttle_block
                | within_block | budget_block | try_block | saga_block | during_block | states_block ;

if_block        = "if" expr block { "else" "if" expr block } [ "else" block ] ;
match_block     = "match" expr "{" [ arm { ( sep | "," ) arm } ] [ sep | "," ] "}" ;
arm             = pattern [ "if" expr ] "=>" ( control | step_body | block | expr ) ;
pattern         = prim_pattern { "|" prim_pattern } ;
prim_pattern    = literal | IDENT | "_" | IDENT "is" TYPE_NAME | record_pattern ;
record_pattern  = "{" field_pat { "," field_pat } [ "," ".." ] "}" ;
field_pat       = IDENT [ ":" ( pattern | IDENT ) ] ;

fork_block      = "fork" [ options ] "{" ⟦ branch ⟧ "}" ;
race_block      = "race" [ options ] "{" ⟦ branch ⟧ "}" ;
branch          = NAME [ "if" expr ] block ;

map_block       = "map" IDENT "in" expr [ options ] block ;
pipeline_block  = "pipeline" IDENT "in" expr [ options ] "{" ⟦ stage ⟧ "}" ;
stage           = "stage" NAME [ options ] block ;
worklist_block  = "worklist" IDENT "in" expr [ options ] block ;

loop_block      = "loop" [ options ] block ;
while_block     = "while" expr [ options ] block ;
for_block       = "for" IDENT "in" expr { "carry" IDENT [ ":" type ] "=" expr } block ;

with_block      = "with" ( "lock" | "semaphore" ) expr [ options ] block ;
throttle_block  = "throttle" "(" rate { "," option } ")" block ;
rate            = expr "per" expr ;
within_block    = "within" expr block [ "else" block ] ;
budget_block    = "budget" options block { "on" ( "soft" [ "(" expr ")" ] | "exhausted" ) block } ;

try_block       = "try" block { "catch" error_pattern [ "as" IDENT ] block } [ "finally" block ] ;
saga_block      = "saga" [ options ] block ;
during_block    = "during" block { "on" event_pattern [ "if" expr ] [ "as" IDENT ] block } ;
states_block    = "states" IDENT "{" ⟦ state_item ⟧ "}" ;
```

### A.5.3 Uniform (extension) steps and blocks

```ebnf
uniform_step    = EXT_KEYWORD [ subject ] [ "->" type ] [ options ] [ STRING ] ;
uniform_block   = EXT_KEYWORD [ subject ] [ "->" type ] [ options ] uniform_body { uniform_clause } ;
subject         = AGENT_REF | DOTTED "(" [ args ] ")" | COMMAND | STRING | IDENT "in" expr
                | ( IDENT | DOTTED ) "[" [ expr { "," expr } ] "]"     (* e.g. goals [a, b] *)
                | expr ;
uniform_body    = block | "{" ⟦ section ⟧ "}" ;
section         = NAME [ options ] block                               (* e.g. round { … } *)
                | stmt
                | uniform_clause ;                                     (* hooks may also appear inside the body *)
uniform_clause  = "on" NAME [ "(" [ args ] ")" ] block                (* e.g. on stalled(3) { … } *)
                | "finally" block ;
EXT_KEYWORD     = IDENT ;                                              (* registered by a used extension *)
```

### A.5.4 Modifiers

```ebnf
modifier        = "place" place_spec
                | "timeout" expr
                | "retry" retry_spec
                | "catch" error_pattern [ "as" IDENT ] block
                | "after" expr ( "keep_waiting" block | "raise" ( DOTTED | IDENT ) | block )
                | "compensate" block
                | "cache" "by" expr [ "for" expr ]
                | "on_error" ( "continue" | "fail" ) ;
place_spec      = IDENT "(" [ args ] ")" ;                             (* host(...), distribute(...), any(...) *)
retry_spec      = INT "x" [ "on" error_pattern ] { "," ( "backoff" backoff | "jitter" ) } ;
backoff         = ( "fixed" | "linear" | "exp" ) "(" args ")" ;
```

### A.5.5 Control

```ebnf
control         = "yield" ( step_body | expr )                      (* step_body when it starts with a step keyword *)
                | "break" [ "with" expr ]
                | "continue"
                | "goto" IDENT
                | "end" [ IDENT ] [ "with" expr ]
                | "cancel"
                | "push" [ "priority" ] expr
                | "restart" "with" record_expr [ options ] [ "at" IDENT ] ;
```

## A.6 State layer

```ebnf
state_decl      = { DOC_COMMENT } [ "initial" ] [ "parallel" ] "state" NAME [ "(" params ")" ]
                  "{" ⟦ state_item ⟧ "}"
                | { DOC_COMMENT } "final" "state" NAME ;
state_item      = var_decl
                | "entry" block | "exit" block
                | "flow" block | "do" step_body
                | transition
                | state_decl
                | "region" NAME "{" ⟦ state_item ⟧ "}"
                | "history" [ "deep" ] ;

transition      = "on" on_trigger [ "if" expr ] [ "as" IDENT ] ( "->" target [ "(" "reenter" ")" ] [ block ] | block )
                | "if" expr "->" target [ block ]
                | "after" expr ( "->" target [ "(" "reenter" ")" ] [ block ] | block ) ;
on_trigger      = event_pattern | "done" | "failure" [ "catch" error_pattern ] ;
root_transition = transition ;                                         (* machine level: implicit root state *)
target          = state_path [ "(" [ args ] ")" ] | "(" state_path { "," state_path } ")" ;
state_path      = NAME { "." NAME } [ "." "history" ] ;
```

## A.7 Expressions

```ebnf
expr            = lambda | ternary ;
lambda          = IDENT "=>" expr ;                                   (* only valid as a call argument *)
ternary         = coalesce [ "?" expr ":" expr ] ;
coalesce        = or_expr { "??" or_expr } ;
or_expr         = and_expr { "or" and_expr } ;
and_expr        = not_expr { "and" not_expr } ;
not_expr        = "not" not_expr | comparison ;
comparison      = additive [ ( "==" | "!=" | "<" | "<=" | ">" | ">=" | "in" | "not" "in" ) additive
                           | "is" type ] ;
additive        = multiplicative { ( "+" | "-" ) multiplicative } ;
multiplicative  = unary { ( "*" | "/" | "%" ) unary } ;
unary           = "-" unary | postfix ;
postfix         = primary { "." NAME [ "(" [ args ] ")" ]           (* call paren: no whitespace before "(" *)
                          | "?." NAME                                (* short-circuits the rest of the chain *)
                          | "[" expr "]"
                          | "[" [ expr ] ":" [ expr ] "]"
                          | "(" [ args ] ")" } ;
primary         = literal | NAME | TYPE_NAME "." NAME | AGENT_REF | "(" expr ")"
                | list_lit | record_expr | "await" IDENT ;           (* NAME: input, event, run, budget, … *)
literal         = INT | FLOAT | DURATION | STRING | RAW_STRING | COMMAND | "true" | "false" | "null" ;
list_lit        = "[" [ expr { "," expr } [ "," ] ] "]" ;
record_expr     = "{" [ field_init { "," field_init } [ "," ] ] "}" ;
field_init      = ( NAME | STRING ) ":" expr | NAME ;
args            = arg { "," arg } ;
arg             = [ NAME ":" ] expr ;
options         = OPT_PAREN option { "," option } [ "," ] ")" ;
option          = NAME ":" option_value ;
option_value    = step_body                                            (* option kind "step", e.g. notify: tool … *)
                | record_type                                          (* option kind "type", e.g. form: { base: string = "main" } *)
                | NAME "in" expr                                       (* e.g. vary: model in [...] *)
                | rate
                | expr ;                                               (* bare NAMEs resolve against the option's enum *)
                (* The parser keeps an option value as an unresolved term and the checker   *)
                (* resolves it by the option's declared kind (core schema or manifest).     *)
event_pattern   = DOTTED | IDENT | PATTERN ;
error_pattern   = "any" | err_alt { "|" err_alt } ;
err_alt         = DOTTED | IDENT | PATTERN ;
```

**Disambiguation:**
- **`{`** opens a `record_expr` only in expression position. After a statement keyword, a block head or an arrow, it opens a block.
- **A step binding vs an assignment.** `IDENT "=" …` is a step binding when the right-hand side starts with a step keyword, a flow call or `async`/`await`. Otherwise it is an assignment, which requires `IDENT` to be a `var`.

## A.8 Templates (inside `STRING`)

```ebnf
template        = { text | interp | tag } ;
interp          = "{{" expr { "|" IDENT [ "(" [ args ] ")" ] } "}}" ;
tag             = "{%" [ "-" ] tag_body [ "-" ] "%}" ;
tag_body        = "if" expr | "else" "if" expr | "else" | "for" IDENT "in" expr | "end" ;
text            = { char | "\{{" } ;
```

## A.9 Tests

```ebnf
test_def        = "test" STRING "{" ⟦ test_stmt ⟧ "}" ;
test_stmt       = "given" ( "input" record_expr | "event" ( DOTTED | IDENT ) [ record_expr ]
                          | "entity" record_expr | "var" IDENT "=" expr )
                | "mock" mock_kind mock_subject "returns" mock_value
                | "send" ( DOTTED | IDENT ) [ record_expr ]
                | "advance" expr
                | "expect" expectation ;
mock_kind       = "agent" | "tool" | "run" | "approve" | "ask" | "call" "machine" | NAME ;  (* NAME: extension effect kind, e.g. assess *)
mock_subject    = AGENT_REF | DOTTED | COMMAND | IDENT | "*" ;
mock_value      = expr | "error" ( DOTTED | IDENT ) | "sequence" "[" mock_item { "," mock_item } "]" ;
mock_item       = expr | "error" ( DOTTED | IDENT ) ;
expectation     = "end" IDENT
                | "waiting" "at" IDENT
                | "in_state" STRING
                | mock_kind mock_subject [ "not" ] "called" [ INT "times" ] [ "with" record_expr ]   (* fields may be matchers, §10.6 *)
                | expr ;
```
