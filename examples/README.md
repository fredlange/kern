# Examples

Each directory is a standalone Kern project with its own README: what it does, how to check
and run it, what it shows, and what `kern check` says about it today.

| Project | What it is | Shows |
|---|---|---|
| [ledger-tour/](ledger-tour/) | one ledger in seven versions, each fixing what the last proof page found | start here: refutations, counterexamples, spec strength, `why` staleness |
| [ledger-large/](ledger-large/) | seven ledger operations in a call chain four deep | modular proofs, inferred interfaces, the editor view |
| [escrow/](escrow/) | a marketplace escrow in three sizes (10, 26 and 55 definitions) | branches, declared state and invariants, `need as`, pure functions |
| [shop/](shop/) | catalogue, discounts and orders; a package of four files | packages and imports, loops, invariants |
| [library/](library/) | books, loans and fines; a package of four files | time in specs, loops, calls across files |
| [lottery/](lottery/) | tickets, a loyalty bonus and a random draw; a package of four files | assumed handlers in a package, replay |
| [limits/](limits/) | withdrawals limited by an outside risk service over HTTP | the network as an effect, replies as calls, the permission manifest |
| [batch/](batch/) | sweeps, subscription renewals and installments over a range | bounded loops with invariants |
| [raffle/](raffle/) | a random draw from a pot | the smallest assumed handler |
| [random-portal/](random-portal/) | a React page of jokes and puns from public APIs, with votes; Docker image | a Kern backend for a web app, real HTTP replies as calls, the proof page served by the app |

Every project has a `proof-pages/` folder with its generated proof pages. Regenerate one with
`kern check FILE --html proof-pages/NAME.html`.

Tests, validation tools and the frozen experiments in this repository read some of these files
by path, so keep their names.
