# todo: a task list with deadlines

The first app outside the ledger domain: tasks keyed by id, with a status (1 open, 2 done), a deadline and
a priority (1–3), all `state` maps. Written from docs/REFERENCE.md and the CLI only.

```
todo/
├── todo.k              add_task, complete, reschedule (guards, clock.now, invariants, emit)
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

## What `check` says (recorded at 2962625, `--cache ''`)

| definition | status | spec strength |
|---|---|---|
| `add_task` | All guarantees proven | 21/21; spec refusals 10/10 |
| `complete` | All guarantees proven | 5/5; spec refusals 2/2 |
| `reschedule` | All guarantees proven | 3/3; spec refusals 4/4 |
| `count_overdue` | All guarantees proven (`result >= 0`, `result <= hi - lo`) | 0/0, nothing in the body to mutate |
| `report_overdue` | All guarantees proven (`emit Overdue (count_overdue lo hi)`) | 3/3 |

`kern build` compiles both files.

## What it can't say yet

See `.party/requests.md`: no `Text` (tasks have no titles), no accumulators in loops, no nested loops.
