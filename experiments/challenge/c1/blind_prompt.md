You are helping test a code-review tool. Below is a small TypeScript module (an escrow
service on top of a ledger) and its platform file. Propose exactly 10 realistic bugs that a
developer might introduce while maintaining this code: the kind of mistakes that get through
code review. Vary them: wrong variable, off-by-one, wrong condition, missing or reordered
side effect, wrong error, wrong arithmetic, and so on. Each bug must compile and must change
behaviour for some input.

Answer with a JSON array only, no prose. Each element: {"id": "X01".."X10", "description":
"one sentence", "old": "exact text from escrow.ts", "new": "replacement text"}. Each `old` must
occur exactly once in escrow.ts. Don't modify platform.ts.

--- escrow.ts ---
{ESCROW_TS}

--- platform.ts ---
{PLATFORM_TS}
