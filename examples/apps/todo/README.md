# todo: a task list with deadlines

The first app outside the ledger domain: tasks keyed by id, with a status (1 open, 2 done), a deadline and
a priority (1–3) and a `Text` title, all `state` maps. Written from docs/REFERENCE.md and the CLI only.

```
todo/
├── todo.k              add_task (with a Text title), complete, reschedule, retitle, mark_urgent (Text `+`, `length`)
├── overdue.k           count_overdue (a sum over a range of ids) and report_overdue
└── proof-pages/        todo.html, overdue.html
```

## Run it

From the repository root (or with `kern` installed):

```text
cargo run --release -p kern -- check examples/apps/todo/todo.k
cargo run --release -p kern -- check examples/apps/todo/overdue.k
cargo run --release -p kern -- build examples/apps/todo/overdue.k
```

## What `check` says (recorded at 59c9e66, `--cache ''`)

| definition | status | spec strength |
|---|---|---|
| `add_task` | All guarantees proven | 24/24; spec refusals 14/14 |
| `complete` | All guarantees proven | 5/5; spec refusals 2/2 |
| `reschedule` | All guarantees proven | 3/3; spec refusals 4/4 |
| `retitle` | All guarantees proven | 1/1; spec refusals 4/4 |
| `mark_urgent` | All guarantees proven (`title' = "!" + title`, length + 1) | 1/1; spec refusals 2/2 |
| `count_overdue`, `report_overdue` (overdue.k) | All guarantees proven | 0/0; 3/3 |

`kern build` compiles both files (Text since e7df085). Writing Text specs needs parentheses on every compound
argument: `length (title.get' id)`, `"!" + (title.get id)`.

## What it can't do yet

See `.party/requests.md`: no accumulators in loops.
