# Engineering standards

`CLAUDE.md` explains *why* each subsystem is built the way it is. `CONTRIBUTING.md`'s Conventions
section states the handful of rules that apply everywhere. This document is the third kind of
guidance neither of those is shaped for: how to keep a module from becoming unreadable as it grows,
how the vanilla-TS view layer stays reusable, how `styles.css` stays navigable, and how a refactor
is scoped and landed. It exists because the codebase has grown past the point where "how we do
things" fits in a bullet list, and because a large, real refactor needs one place that says where
to draw the line before someone (or some agent) draws it differently in every file it touches.

Follow this document for any structural change. It does not replace the shorter Conventions
sections — read those first.

## What this project deliberately does not do

Two things were proposed, discussed, and rejected while writing this document. They are recorded
here so neither is re-proposed by someone who has not seen the reasoning:

- **No React, no Redux, no other frontend framework.** `src/dom.ts` says why: four screens and a
  list that changes on refresh do not justify a runtime dependency in a VPN client whose whole
  pitch is a small, auditable surface — "every shipped byte is something a user has to trust."
  Adopting one now would mean rewriting the ~12,000 lines of view and controller code in `src/` and
  adding a real dependency tree to trust, for a state-management problem `store.ts` already solves
  without one (see below). Nothing here forbids revisiting this later, but that is a decision for
  the maintainer, made on its own, not folded into an unrelated change.
- **No Tailwind, no CSS-in-JS.** `styles.css` is 3082 lines in one file, but the problem is
  organization, not the tool: a token layer already exists at the top of the file and is read back
  live by `design/styleguide.ts`. Tailwind would mean a new build dependency and rewriting every
  element's styling app-wide to fix a problem that section headers and stricter token discipline
  already fix for free. See *Styling* below.

## State: one store, one direction

`store.ts` already implements the pattern this section names. Every mutation goes through
`store.update()`, which persists (via `persist.ts`) and notifies subscribers in the same call, so a
view can never change state that the rest of the app does not hear about. This *is* the project's
Redux — one store, one-way data flow, subscriber notification — without a library, because the
whole app has exactly one store and has never once needed reducer or middleware ceremony to go with
it.

Rules that keep this true as the app grows:

- **Views never mutate a `Server`, `Group`, or `Settings` object in place.** Always go through a
  `store.*` method that itself calls `update()`. Reaching in — `store.servers[i].latency = …` — is a
  bug the next `refresh()` will silently overwrite.
- **Derived data is a function, not a cached field.** `located()`, `quickPicks()`, `groupPlaces()`
  are the model: compute from a snapshot on read. A `Server` field is only justified when it is
  itself a measurement being persisted (`exit`, `entry`, `usage`), not a value derived from other
  fields.
- **Session-only state stays out of the store, by name, not by accident.** `main.ts` already keeps
  `edges`, `tunnelEpoch`, and `tunnelFault` as plain module-level variables rather than store fields,
  because CLAUDE.md documents each as deliberately non-persisted: `edges` would leak which network
  an exit was observed on if written to disk and replayed; `tunnelEpoch` exists only to discard
  answers that were in flight across a connect/disconnect; `tunnelFault` is cleared only by a new
  attempt, never restored from a file. When new runtime-only state is needed, keep it in the
  smallest module that owns the behaviour, and say in a comment why it isn't in the store — putting
  it there implies it should survive a restart, and this class of state shouldn't.
- **A derived answer two views need is a store export, not two copies.** If a second view starts
  needing what another already computes, move the computation next to the data in `store.ts` and
  import it, rather than recomputing a slightly different version in place.

## Splitting a large module

`src/main.ts` (2863 lines) and `src/store.ts` (777 lines) are the two frontend files past a single
comfortable scroll. `store.ts` is one coherent concern — the data model — and reads fine at that
length; it is not a splitting candidate below roughly 1200 lines. `main.ts` is a different case.

**A module is a candidate for splitting once it mixes more than one reason to change — a line count
is a symptom, not the test.** `main.ts` is already cut into commented sections (app state, views,
rendering, popover, tunnel, blocking, add servers, sheets, rail icons, boot), and even those have
drifted from one concern each: the block under `// ---- blocking ----` (`src/main.ts:1403`–1679)
contains not just `syncBlockLists`/`paintBlocking`/`watchBlockSwitches`, but also `watchMode`, the
throughput-polling loop (`poll`/`sample`/`readCounters`/`saveUsage`), `refreshReadiness`, and
`refreshSubscription` — five different reasons to change hiding under one banner. A comment is not
a module boundary; only an import statement is.

**Guidance, not a hard rule:** treat ~600 lines as the point to start looking for a seam, and
~1200 as a file that should already have one. When `main.ts` is split (as its own sequence of
issues, per *How a refactor lands* below), each extraction:

- Becomes its own `src/features/<name>.ts`, importing `store` directly — this is a housekeeping
  split, not a redesign of how state flows.
- Is scoped and verified on its own: pull one real reason-to-change out (starting with the
  `tunnel` section at `src/main.ts:1197`–1402, which is already genuinely single-purpose —
  connect/disconnect/reconnect and the system-proxy sequencing — and is the safest first cut because
  nothing else depends on how it's split), confirm behaviour with `VITE_MOCK=1 npm run dev`, then
  move to the next. A PR that splits a file *and* changes its behaviour is two changes — open two
  PRs.
- Leaves `main.ts` holding only boot, `show()`/`refresh()`, and the tray/rail/popover sync that
  wires the extracted pieces together — what a file named `main.ts` should actually contain.

The same reason-to-change test applies on the Rust side, where `subscription.rs` (1914 lines),
`config.rs` (1495) and `geo.rs` (1345) are each already larger than `main.ts`'s worst section but,
unlike it, each is presently one coherent concern (parsing subscription bodies; building one config
shape; locating addresses). Don't split a Rust module to hit a number — split it when it earns a
second reason to change. `subscription.rs` is the nearest future candidate: it already separates by
comment into fetching, the three `config_links` readers (Xray/sing-box/Clash), and the one writer,
a shape close enough to `main.ts`'s that the same extraction approach would apply if it grows
further.

## Reusable views over the DOM helper

`dom.ts`'s `h`/`render`/`qs` is deliberately not a framework, but a component here is still just a
function (or small class) that takes plain data and returns a `Node` — the same shape a React
component has, without JSX or a virtual DOM. `views/status.ts`'s `StatusCard` is the pattern to
copy: its `render(model: StatusModel)` takes a plain object and touches no store, which is exactly
what lets `design/styleguide.ts` mount the *real* component instead of a hand-copied fixture.

Every other view currently subscribes to the store directly in its own constructor — which is why
`design/README.md` already flags the one place this codebase is allowed to drift:
`design/styleguide.ts`'s `locRow` duplicates `views/locations.ts`'s row markup by hand, because the
real row can't be mounted without the store. The two will disagree the moment one gains an element
the other doesn't.

Going forward:

- **New or refactored views take a plain model and callbacks, `StatusCard`-shaped**, even where
  passing a whole `Server`/`Group` would be convenient — pass the specific fields the view actually
  reads, so it can be mounted from a fixture or the style guide without the store. Keep the
  store-subscribing wiring one layer up, in the `main.ts`/`src/features/*` module that owns it.
- **A markup pattern used in two places becomes a function, not a second hand-copy.** Extract it
  (e.g., export a `locRow`-shaped helper from `views/locations.ts` for the style guide to import)
  instead of letting a second copy exist to drift.
- **Give a reusable view its own named `*Model`/`*Props` type**, `StatusModel`'s shape, rather than
  an inline object literal repeated at each call site.
- **A view file owns one visual area.** `views/locations.ts`, `views/map.ts`, `views/status.ts`
  each cover one part of one screen; when a second area's code ends up in one by convenience, that
  is a sign to add a file, not grow the existing one.

## Styling: tokens over literals, sections over one long file

The fix for a 3082-line `styles.css` is organization, not a new tool (see *What this project
deliberately does not do*). The token layer to organize around already exists at the top of the
file (`--ground`, `--ink`, `--brand`, `--live`, `--warn`, `--off`, the map's land/border tokens) and
is exactly what `design/styleguide.ts` reads back through the CSSOM — the style guide is already
the enforcement mechanism for the first rule below, since an untokenized colour shows up there
ungrouped, under "Other."

- **Never write a literal colour, shadow, or radius in a component rule.** If the value isn't a
  token yet, add it to `:root` and its dark-mode override first, then reference it — that's what
  makes it visible in the style guide instead of silently drifting from the palette.
- **Group `styles.css` by the same section-banner convention `main.ts` uses**
  (`/* ---- name ---- */`), one banner per component or screen area, in the order those areas
  appear in the app. The file doesn't need splitting into several `.css` files — Vite bundles one
  stylesheet regardless, and `popover.css` already shows the cost of a second file: its `--margin`
  has to be kept in step by hand with Rust's `MARGIN` constant — but it does need contiguous,
  findable sections, which it presently lacks past the token block.
- **A new component's rules get a new banner**, not an append to whichever section happens to sit
  nearby in the file.
- **Dark mode is a token override, never a duplicated rule.** Every colour token gets a light value
  at `:root` and a dark value under the existing `prefers-color-scheme: dark` block; component rules
  reference the token and never re-declare a colour per theme.

## Rust: conventions already in force, written down here for the first time

The Rust side already follows the standard this section names — it was only ever written as prose
inside each module and inside `CLAUDE.md`'s per-subsystem sections, not as a rule on its own:

- **One module per subsystem, named for what it owns, not for a layer.** `sysproxy.rs`, `geo.rs`,
  `subscription.rs` — never `utils.rs` or `helpers.rs`, which have no reason of their own to change
  and become a dumping ground for things that do.
- **Every module opens with a rationale comment**: the decision made and the alternative rejected.
  This is the single most-repeated rule in this codebase's own documentation, and it applies to
  every `.ts` file exactly as much as every `.rs` one. A module that arrives without one reads as
  foreign here.
- **A function that can fail returns `Result`, with an error specific enough to act on** — see
  `sysproxy.rs` treating `networksetup`'s `** Error` output as failure despite its exit code of 0 —
  never a bare boolean or a logged-and-swallowed error.
- **A test needing more than a checkout is `#[ignore]`d**, with a comment saying what it needs (a
  live core, `CAP_NET_ADMIN`, a real Mac's `networksetup`), so the default `cargo test` run stays
  fast and pure.
- **No `cargo fmt` gate.** The tree is not rustfmt-clean; do not reformat a file you are not
  otherwise editing, even in the middle of refactoring something else in it.

Size guidance mirrors the frontend's, with the same caveat that a reason-to-change test beats a
line count — see the note on `subscription.rs`/`config.rs`/`geo.rs` under *Splitting a large
module* above.

## Testing

- **Test names are sentences**, in both languages —
  `the_tunnel_takes_the_default_route_and_gives_it_back` on the Rust side, the same style for any
  new frontend `*.test.ts` title.
- **A module meant for `npm test` coverage imports nothing with a runtime value from `store.ts`.**
  Type-only imports are erased by Node's own type stripping, which is why `usage.ts`, `identity.ts`,
  `quick.ts`, `share.ts`, `serial.ts`, `shield.ts`, `support.ts`, `edge.ts` and `blocking.ts` all
  stand apart from the store already. Any new pure-logic module extracted from `main.ts`
  (`src/features/*`, above) keeps this property if it is to be tested at all: store-free, taking the
  fields it needs as arguments.
- **A round-trip property gets a test that asserts the round trip, not each direction separately**
  — `parseShareLink(toShareLink(p)) === p` and `parseWgQuick(toWgQuick(p)) === p` are the existing
  shape; a new serialization follows it.

## How a refactor lands

A refactor follows the same path as any other change (`CONTRIBUTING.md`, *How work is done*): one
GitHub issue per area, a linked `feature/…` or `bugfix/…` branch, a PR that closes the issue, and a
squash merge into `main`. **An issue is scoped to one file or one extraction**, never "clean up the
frontend" — the `main.ts` split above is deliberately a sequence of separate cuts for this reason,
each one scoped and reviewed on its own once it's its turn, not pre-decided in bulk here.

Every refactor PR runs the existing checks before merge — `npm run build`, `npm test`,
`cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings`,
`cargo test --manifest-path src-tauri/Cargo.toml` — and, for anything touching a screen,
`VITE_MOCK=1 npm run dev` by hand. A refactor that only moves code and still fails a test moved the
bug along with it, not just the code.
