# Architecture

How the app is put together: what each part does, where its data comes from, and what it keeps. Read this before the code. The README covers building and the git hooks.

## The shape

```
                 ┌────────────────────────── crates/app (egui) ──────────────────────────┐
                 │  pages: Today, What to build, Build, Stockpiles, Assets, Orders, …    │
                 │  one background worker thread does every download and file read       │
                 └──────┬──────────┬──────────┬──────────┬──────────┬──────────┬─────────┘
                        │          │          │          │          │          │
                      esi        market      sde       jeve       store       core
                (login, your  (Jita and   (game's    (jEveAssets (your own  (math: money,
                 characters)  Forge       static     profile,    data in    builds, fees,
                              prices)     data)      read-only)  SQLite)    profit, stock)
```

Every crate except `app` is plain Rust with no UI. `core` depends on nothing; the others depend on `core`, and `store` also on `market` and `sde`. Only `app` uses egui. A different front end would replace `crates/app` and keep the rest.

## The crates

| Crate | Package | What it does |
|---|---|---|
| `core` | `mop-core` | The calculations: money and rates as exact integers, build and invention costs, job time, fees, freight, make-or-buy, profit, buy lists, stock and stockpiles, reservations, net worth. No files, no network. |
| `esi` | `mop-esi` | EVE's single sign-on (login page, PKCE, tokens) and the character calls: assets, blueprints, industry jobs, skills, wallet, places. Long-lived login keys go to the system keychain, or a plain file when there is none. |
| `market` | `mop-market` | Prices. Jita order books and quotes from ESI, cached by ESI's own cache and rate-limit headers. The Forge's whole-region summary from Fuzzwork. The Forge's daily sales history from ESI. No clock inside: callers pass the time in. |
| `sde` | `mop-sde` | The game's static data (item names, volumes, recipes, invention, structures and rigs, the map), downloaded from EVE's developer site and imported into a local database. |
| `jeve` | `mop-jeve` | Reads a jEveAssets profile file: its characters, assets, blueprints and stockpiles. Read-only. The file also holds login tokens; the reader never selects them. |
| `store` | `mop-store` | Everything the user owns or set up: plans, facilities, freight routes, settings, stockpiles, reservations, blueprints, wallet journal and transactions, the change log, net worth history and price history. One SQLite file, one transaction per method. |
| `app` | `eve-means-of-profit` | The desktop program: pages, panels, layouts, the first-run setup, and the worker thread. |

## Where data comes from

| Source | What | Crate |
|---|---|---|
| ESI (CCP's API), after login | Your characters' assets, orders, jobs, blueprints, skills, wallet | `esi` |
| ESI, public | Jita order books, The Forge's daily history | `market` |
| Fuzzwork (`market.fuzzwork.co.uk`) | The Forge's summary prices for every item | `market` |
| EVE developer site (static data export) | Items, recipes, map, structures | `sde` |
| jEveAssets on the same machine | An existing profile, so you don't start from nothing | `jeve` |

Nothing is sent anywhere else. There is no server: all data stays on your machine.

## Where it lives on disk

In the system's data folder for `eve-means-of-profit` (on Linux `~/.local/share/eve-means-of-profit/`):

| File or folder | What |
|---|---|
| `user.sqlite` | The `store` database: your data. |
| `sde.sqlite` | The imported static data. It can always be downloaded again. |
| `esi/` | Login data for your characters. |
| `images/` | Item icons and character portraits from `images.evetech.net`. |
| `layouts.txt` | Your saved page layouts. |

Until the first release, `user.sqlite` has no upgrade steps: when its stored layout differs from the one the program expects, the old file is set aside and a fresh one is made. Numbered upgrade steps start at the first release.

## How a refresh flows

1. A page asks for something (a refresh button, the start of the app). The UI sends a job to the worker.
2. The worker thread runs jobs one at a time, in order. It does the network calls and file reads, so a slow download never freezes the window.
3. Each job sends its result back over a channel and asks the window to redraw.
4. Pages only read what the worker already loaded. No page fetches from EVE itself.

## What is kept, and for how long

One rule decides it: **keep for good what can never be fetched again; thin out what can be fetched again.**

Kept for good:
- **What you held.** For assets, orders and jobs, the store keeps each character's latest state and appends only what changed at each refresh: what appeared, what changed, what is gone, by EVE item id (`crates/store/src/changes.rs`). An unchanged refresh adds nothing.
- **Net worth over time.** Each record keeps the wallet, sell orders, buy escrow, and the items it counted. Prices are kept as one reading per item per day.
- **Your wallet journal and transactions.**

Thinned or refetched:
- **Market history** for the markets you chose: one reading a day, kept for as long as that market's retention setting says.
- **The static data** and the cached order books, which can always be downloaded again.

What ESI cannot tell us: when two stacks merge, EVE gives the result a new item id, so the change log sees one stack gone and a new one appear. Where an item came from (bought, built, looted) is not in the data, and the app does not guess.

## Number rules

- **Money is an `i64` count of ten-thousandths of ISK** (0.0001 ISK). Multiplication goes through `i128`, and rounding happens only for display. No floats for money.
- **Arithmetic is checked.** An out-of-range result is an error, never clamped to a limit.
- Times are passed in by the caller wherever possible, so the calculations can be tested without a clock.

## Tests

Each crate has its own tests (`crates/<name>/tests/` and unit tests next to the code). The UI is tested with `egui_kittest`, including snapshot images of panels. `cargo test` runs everything; the git hooks in the README run formatting, clippy and the tests on commit and push.
