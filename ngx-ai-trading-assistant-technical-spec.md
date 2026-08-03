# NGX AI Trading Assistant — Technical Specification

**Version:** 1.0 (Sandbox / Mock-Execution Phase)
**Author:** Dispatch-Z
**Status:** Draft for implementation

---

## 1. Overview

### 1.1 Purpose
An AI-powered trading assistant that ingests live and historical Nigerian Exchange (NGX) market data, generates trade signals using a hybrid technical + fundamental + LLM-reasoning pipeline, and autonomously executes **mock trades** against a simulated portfolio in a sandboxed environment. No real capital or brokerage integration is in scope for this phase.

### 1.2 Goals
- Prove out signal-generation quality and strategy logic before any real capital or broker integration is considered.
- Build a realistic sandbox execution engine (fills, slippage, fees, portfolio accounting) so results are a meaningful proxy for live performance.
- Produce a fully autonomous loop: ingest → analyze → decide → execute (mock) → record → report, running on a schedule with no human in the loop, constrained by a fixed, auditable parameter set.
- Establish a clean seam so a real broker/execution adapter can be swapped in later without re-architecting the system.

### 1.3 Non-Goals (this phase)
- No real order placement, no broker/dealer integration.
- No handling of real client funds.
- No multi-user / multi-tenant portfolio support (single sandbox portfolio to start).

### 1.4 Explicit Assumptions
- Market data source: NGX Pulse API (`https://www.ngxpulse.ng/api`), **Personal (free) tier** — 100 requests/day, 10 requests/min. See §5 for how ingestion is scheduled to stay inside this budget.
- Mock capital: a fixed configurable starting balance (e.g. ₦10,000,000), held in a `sandbox_portfolios` ledger — no real money anywhere in the system.
- "Autonomous" means the signal engine's decision is executed immediately in the sandbox without a human approval step, since no real capital is at risk. (Note: this differs from the earlier human-in-the-loop design discussed for live trading — that approval gate should be re-introduced before any real-money phase.)
- Trading universe: NGX main-board equities to start; indices used as context/benchmark, not directly traded.

---

## 2. High-Level Architecture

```
┌────────────────────────────────────────────────────────────────────┐
│                         Scheduler / Orchestrator                     │
│              (NestJS cron + Redis-backed job queue — BullMQ)         │
└───────────────┬───────────────────────────────────┬─────────────────┘
                │                                   │
                ▼                                   ▼
┌───────────────────────────────┐   ┌───────────────────────────────────┐
│  1. Market Data Ingestion       │   │  5. Reporting / Analytics Jobs      │
│  Service (NestJS)                │   │  (daily P&L snapshot, drawdown,    │
│  - Polls NGX Pulse endpoints     │   │   win-rate, benchmark vs ASI)      │
│  - Normalizes + writes to        │   └───────────────────────────────────┘
│    Supabase                      │
│  - Caches hot data in Redis      │
└───────────────┬───────────────┘
                │
                ▼
┌───────────────────────────────┐
│  2. Signal Generation Engine    │
│  (NestJS service + Langchain)   │
│  - Technical indicator compute  │
│  - Fundamental + news context   │
│  - LLM reasoning → signal        │
│  - Writes signal + rationale     │
│    to Supabase                  │
└───────────────┬───────────────┘
                │
                ▼
┌───────────────────────────────┐
│  3. Strategy / Risk Policy      │
│  Engine (NestJS, deterministic) │
│  - Applies fixed parameter set  │
│  - Position sizing               │
│  - Stop-loss / exposure caps     │
│  - Approves or blocks a signal  │
│    from reaching execution       │
└───────────────┬───────────────┘
                │
                ▼
┌───────────────────────────────┐
│  4. Sandbox Execution Engine    │
│  (NestJS)                        │
│  - Simulates order fill at       │
│    current/next price + slippage │
│  - Updates mock portfolio ledger │
│  - Writes trade + position        │
│    records to Supabase            │
└───────────────┬───────────────┘
                │
                ▼
┌───────────────────────────────┐
│  6. Dashboard (Next.js)          │
│  - Live portfolio view            │
│  - Signal + trade history         │
│  - Performance charts              │
│  - Strategy parameter config UI    │
└───────────────────────────────┘
```

All services can live in a single NestJS monorepo (modular monolith) for this phase — no need for separate microservices yet. Recommended module boundaries mirror the six components above.

---

## 3. Technology Stack Mapping

| Layer | Technology | Notes |
|---|---|---|
| Backend framework | NestJS | Modular monolith; one module per component above |
| Frontend | Next.js | Dashboard, config UI, reporting views |
| Database | Supabase (Postgres) | Source of truth: prices, signals, trades, portfolio, config |
| Cache / Queue | Redis (+ BullMQ) | Hot price cache, job scheduling, pub/sub for live dashboard updates |
| AI reasoning | OpenAI + Langchain | Signal rationale generation, structured output parsing |
| Market data | NGX Pulse API | Stocks, indices, fundamentals, disclosures, news |
| Hosting | (unspecified — assume existing Knuckle AI infra or new service) | |

---

## 4. Data Model (Supabase / Postgres)

### 4.1 `instruments`
Reference table of tradable symbols.

| Column | Type | Notes |
|---|---|---|
| symbol | text PK | e.g. `DANGCEM` |
| name | text | |
| sector | text | |
| is_active | boolean | whether the strategy is allowed to trade this symbol |
| added_at | timestamptz | |

### 4.2 `price_history`
Time-series OHLCV / snapshot data pulled from NGX Pulse.

| Column | Type | Notes |
|---|---|---|
| id | bigint PK | |
| symbol | text FK → instruments | |
| trade_date | date | |
| price | numeric | current/close price |
| change_percent | numeric | |
| volume | bigint | |
| market_cap | numeric | |
| pe_ratio | numeric | |
| source_updated_at | timestamptz | timestamp from NGX Pulse response |
| ingested_at | timestamptz default now() | |

Indexed on `(symbol, trade_date)` unique — upsert on ingestion.

### 4.3 `index_history`
| Column | Type | Notes |
|---|---|---|
| id | bigint PK | |
| index_code | text | e.g. `ASI`, `NGXBNK` |
| trade_date | date | |
| value | numeric | |
| points | numeric | |
| week_change / month_change / year_change | numeric | |

### 4.4 `fundamentals_snapshot`
| Column | Type | Notes |
|---|---|---|
| id | bigint PK | |
| symbol | text FK | |
| snapshot_date | date | |
| eps, dividend_per_share, dividend_yield, roe, roa, pb_ratio, debt_equity, beta, profit_margin | numeric | mirrors NGX Pulse fundamentals fields |
| extra | jsonb | EV/EBITDA, PEG, RSI, F-score, analyst consensus, etc. |

### 4.5 `disclosures` / `news`
| Column | Type | Notes |
|---|---|---|
| id | bigint PK | |
| symbol | text nullable | null for market-wide news |
| headline | text | |
| body_summary | text | |
| source | text | |
| published_at | timestamptz | |
| category | text | earnings / dividend / rights-issue / AGM / general |

### 4.6 `signals`
Every decision the AI engine produces — approved or not.

| Column | Type | Notes |
|---|---|---|
| id | uuid PK | |
| symbol | text FK | |
| generated_at | timestamptz | |
| action | text | `BUY` \| `SELL` \| `HOLD` |
| confidence | numeric(3,2) | 0.00–1.00 |
| rationale | text | LLM-generated explanation |
| technical_snapshot | jsonb | indicator values used at decision time |
| fundamental_snapshot | jsonb | fundamentals used at decision time |
| model_name | text | e.g. `gpt-5.x` — record which model produced it |
| prompt_version | text | version tag for the prompt template used |
| risk_policy_result | text | `APPROVED` \| `BLOCKED_EXPOSURE` \| `BLOCKED_STOPLOSS` \| `BLOCKED_OTHER` |
| executed | boolean | whether it resulted in a sandbox trade |

### 4.7 `sandbox_portfolios`
| Column | Type | Notes |
|---|---|---|
| id | uuid PK | |
| name | text | e.g. `default-sandbox` |
| starting_capital | numeric | fixed mock capital |
| cash_balance | numeric | current mock cash |
| created_at | timestamptz | |
| strategy_param_set_id | uuid FK | see 4.10 |

### 4.8 `sandbox_positions`
| Column | Type | Notes |
|---|---|---|
| id | uuid PK | |
| portfolio_id | uuid FK | |
| symbol | text FK | |
| quantity | numeric | |
| avg_cost | numeric | |
| updated_at | timestamptz | |

### 4.9 `sandbox_trades`
| Column | Type | Notes |
|---|---|---|
| id | uuid PK | |
| portfolio_id | uuid FK | |
| signal_id | uuid FK → signals | |
| symbol | text | |
| side | text | `BUY` \| `SELL` |
| quantity | numeric | |
| fill_price | numeric | simulated fill (see §7.2) |
| simulated_fee | numeric | |
| simulated_slippage_bps | numeric | |
| executed_at | timestamptz | |
| resulting_cash_balance | numeric | |

### 4.10 `strategy_param_sets`
The "fixed set of parameters" the autonomous engine runs under. Versioned so backtests and live sandbox runs can be tied to an exact configuration.

| Column | Type | Notes |
|---|---|---|
| id | uuid PK | |
| name | text | |
| max_position_pct | numeric | max % of portfolio in a single symbol |
| max_daily_trades | integer | |
| stop_loss_pct | numeric | per-position stop-loss trigger |
| take_profit_pct | numeric | nullable |
| min_confidence_to_trade | numeric | e.g. 0.65 |
| max_daily_drawdown_pct | numeric | circuit breaker — halts trading for the day |
| allowed_symbols | text[] | nullable = all active instruments |
| created_at | timestamptz | |
| is_active | boolean | only one active set at a time per portfolio |

### 4.11 `daily_performance_snapshot`
| Column | Type | Notes |
|---|---|---|
| id | bigint PK | |
| portfolio_id | uuid FK | |
| snapshot_date | date | |
| total_equity | numeric | cash + market value of positions |
| pnl_daily | numeric | |
| pnl_cumulative | numeric | |
| benchmark_asi_change_pct | numeric | for comparison |
| drawdown_pct | numeric | |

---

## 5. NGX Pulse API Integration

### 5.1 Constraint: Personal tier, 100 req/day, 10 req/min
This phase runs on NGX Pulse's **free Personal tier only** — no fundamentals, disclosures, historical bulk-history, NASD, or forex endpoints (those require Starter+). Everything in this section is designed around what Personal tier actually includes: stock prices and market overview, 100 requests/day, 10 requests/min. The single highest-leverage fact here is that **`GET /api/ngxdata/stocks` returns all 150+ equities in one request** — that one call is worth roughly 150x any per-symbol call, so the whole polling strategy is built around leaning on it instead of per-symbol polling.

### 5.2 Daily request budget

| Purpose | Endpoint | Cadence | Calls/trading day |
|---|---|---|---|
| Full equities snapshot | `GET /api/ngxdata/stocks` | Every 30 min, 9:00–16:00 WAT (14 ticks) | 14 |
| Market overview (ASI, breadth) | `GET /api/ngxdata/market` | Every 60 min (7 ticks) | 7 |
| Index universe | `GET /api/ngxdata/indices` | Every 60 min (7 ticks) | 7 |
| Post-close reconciliation | `GET /api/ngxdata/stocks` | Once, ~16:05 WAT | 1 |
| **Scheduled subtotal** | | | **29** |
| Targeted re-check (`/prices/:symbol`) for top signal candidates only, right before a decision is finalized | `GET /api/ngxdata/prices/:symbol` | On-demand, capped | ≤ 15 |
| Retry/backoff buffer (failed calls, transient errors) | any | as needed | ≤ 10 |
| **Reserved subtotal** | | | **≤ 25** |
| **Total committed** | | | **≤ 54 / 100** |

This leaves roughly **46 requests/day of headroom** — deliberately, not wasted: it absorbs days where retries spike, gives room to tighten the `/stocks` cadence later (e.g. every 20 min = 21 calls instead of 14) without redesigning anything, and — critically — funds historical backfill (§5.4) without ever touching the live-polling budget.

`market-status` is **not polled via the API at all**. NGX's trading calendar (9:00–16:00 WAT, Mon–Fri, minus published public holidays) is stored locally as a static schedule and consulted in-process; this is a zero-cost gate that avoids spending any of the 100-request budget just to ask "is the market open."

### 5.3 Trading universe scoping
Polling the full market via `/stocks` is cheap request-wise (1 call), but the **signal engine** doesn't need to reason over all 150+ symbols every cycle — that's 150+ LLM calls per tick, which is the actual bottleneck (cost and latency), not the NGX Pulse quota. Scope the active trading universe down to a curated list (`allowed_symbols` on the `strategy_param_sets` row, §4.10) — e.g. the 20–30 most liquid main-board names — for signal generation and sandbox trading, while still ingesting and storing the full `/stocks` snapshot for later expansion and market-context purposes.

### 5.4 Historical backfill, without blowing the daily budget
Personal tier has no bulk historical endpoint, so backfill for backtesting relies on `GET /api/ngxdata/prices/:symbol?from=...&to=...` — one call per symbol. Backfilling the full universe in one day (150+ calls) would exceed the daily cap on its own. Instead:
- Backfill only the curated trading universe (§5.3) — 20–30 symbols, not 150+.
- Spread it across **non-trading windows** (weekends, public holidays) when the live-polling budget (§5.2) isn't being spent at all, so the full 100/day is available for backfill instead.
- At ~3–5 symbols backfilled per non-trading day, a 25-symbol universe is fully seeded within a week without ever competing with live polling.
- Track backfill progress in a small `backfill_state` table (`symbol`, `earliest_date_fetched`, `last_run_at`) so the job is idempotent and resumable.

### 5.5 What Personal tier means for fundamentals/disclosures/news
These stay **out of scope** for the signal engine while on Personal tier — the Signal Engine design (§6) should treat the fundamental and contextual layers as optional/nullable inputs rather than assuming they're always present, so the system degrades gracefully to technical-only signals now and gains richer inputs automatically if the tier is upgraded later. No code path should hard-require fundamentals data to exist.

### 5.6 Ingestion service design
- `MarketDataIngestionModule` (NestJS) with a scheduled provider using `@nestjs/schedule` or BullMQ repeatable jobs, configured to the cadences in §5.2.
- Local trading-calendar check (§5.2) gates every job — no API spend outside market hours.
- All responses upserted into Supabase; Redis holds the **latest** snapshot per symbol for fast read access by the signal engine and dashboard (`price:{symbol}` key, TTL slightly longer than poll interval) — this is what lets the dashboard and signal engine read "live" data without ever making their own NGX Pulse calls.
- Rate-limit guard: a Redis-based token bucket hard-capped at 10 req/min and 100 req/day, shared across every NGX Pulse caller in the process (scheduled jobs, on-demand targeted lookups, and backfill jobs all draw from the same bucket) — this is the single enforcement point that keeps the whole system inside the Personal tier limits regardless of which module is calling.
- A `ngx_pulse_usage_log` table (or a simple daily Redis counter) records calls made per day, so the remaining budget is visible on the dashboard rather than discovered via a 429.

---

## 6. Signal Generation Engine

### 6.1 Inputs assembled per symbol, per cycle
1. **Technical layer** (computed in-process from `price_history`): SMA(50), SMA(200), RSI(14), momentum (% change over configurable window), volume anomaly (vs 20-day average).
2. **Fundamental layer**: latest `fundamentals_snapshot` row — P/E, dividend yield, ROE, debt/equity, and the `extra` object (EV/EBITDA, PEG, F-score) where available.
3. **Contextual layer**: relevant `disclosures` and `news` rows from the last 48 hours for that symbol; sector index movement from `index_history`.

### 6.2 Decision pipeline
1. Deterministic pre-filter: skip symbols with no `is_active` flag, insufficient price history (< 60 trading days), or where a position already exceeds `max_position_pct`.
2. Assemble a structured context object (JSON) with all layers above.
3. Call the LLM via Langchain with a fixed prompt template (versioned — see `prompt_version`), instructing it to return **structured JSON only**: `{action, confidence, rationale}`.
4. Validate/parse output against a strict schema (Langchain output parser or Zod validation on the NestJS side); reject and retry once on malformed output, then fall back to `HOLD` with `risk_policy_result = BLOCKED_OTHER` if still invalid.
5. Persist the signal row regardless of outcome (full audit trail, including HOLDs and rejects).

### 6.3 Prompt design principles
- The prompt must instruct the model to reason **only from the supplied data**, not from general market knowledge or memorized information about specific companies, to avoid stale or hallucinated claims.
- Require the model to cite which specific inputs (e.g. "RSI at 78, above overbought threshold") drove the decision, to keep `rationale` auditable and debuggable.
- Confidence must be a calibrated 0–1 score with defined bands (e.g. documented in the prompt: <0.5 = weak conviction, 0.5–0.75 = moderate, >0.75 = strong) so `min_confidence_to_trade` is meaningful.

---

## 7. Strategy / Risk Policy Engine

This is a **deterministic** module — no LLM involved — that sits between signal generation and execution. It is the safety layer, and it operates entirely off the active `strategy_param_sets` row.

### 7.1 Checks applied, in order
1. **Confidence gate**: `signal.confidence >= min_confidence_to_trade`, else block.
2. **Exposure cap**: would this trade push the symbol's position above `max_position_pct` of current total equity? Else block.
3. **Daily trade cap**: has `max_daily_trades` already been reached for this portfolio today? Else block.
4. **Circuit breaker**: has cumulative daily drawdown already breached `max_daily_drawdown_pct`? If so, block all new BUY signals for the remainder of the day (SELL signals to reduce risk may still be allowed — configurable).
5. **Symbol allowlist**: if `allowed_symbols` is set, reject anything outside it.

### 7.2 Position sizing
Default sizing rule (configurable): allocate a fixed percentage of current equity per trade, capped by `max_position_pct`, rather than letting the LLM decide size. Sizing logic and its rationale should be isolated in its own function (`calculatePositionSize`) so it can be swapped for more sophisticated schemes (e.g. Kelly-fraction, volatility-adjusted sizing) later without touching the rest of the pipeline.

---

## 8. Sandbox Execution Engine

### 8.1 Fill simulation
Since there's no real order book access, fills are simulated:
- **Fill price** = last known price from `price_history`/Redis cache at time of execution, adjusted by a configurable `simulated_slippage_bps` (basis points), applied against the trader (worse price on both buys and sells) to keep results conservative rather than optimistic.
- **Fees**: a configurable flat or percentage fee per trade (mirroring typical NGX brokerage commissions + SEC/NGX levies), so mock P&L isn't artificially inflated versus a real execution.

### 8.2 Ledger mechanics
- BUY: decrease `cash_balance`, increase/create `sandbox_positions` row, recompute `avg_cost`.
- SELL: increase `cash_balance`, decrease/close `sandbox_positions` row, realize P&L into the trade record.
- Every mutation is a single Postgres transaction (Supabase) covering the trade insert + portfolio/position updates, to avoid partial-write inconsistency.

### 8.3 Autonomous loop
Given this phase has no human approval step, the full cycle (ingest → signal → risk policy → sandbox execution → snapshot) should run as one orchestrated job per polling interval, with each stage's output persisted independently — so a failure at any stage doesn't silently lose data, and each stage is independently replayable/debuggable.

---

## 9. Backtesting Framework

Distinct from the live sandbox loop — runs against historical data to validate a `strategy_param_set` before it's promoted to run autonomously.

- Input: a historical date range, using `price_history`/`index_history` (NGX Pulse index history goes back to 1996; equity price history to 2017) plus historical `fundamentals_snapshot` where available.
- Replays the same Signal Engine + Risk Policy + Sandbox Execution logic day-by-day (or at whatever granularity data supports) against a fresh mock portfolio, without calling the live LLM for every historical day if cost is a concern — consider caching LLM outputs per (symbol, date, prompt_version) so repeated backtest runs against the same data don't re-spend on inference.
- Output: same `daily_performance_snapshot` shape as live sandbox, plus standard metrics: total return, max drawdown, Sharpe-like ratio (using available data), win rate, benchmark comparison against ASI change over the same period.

---

## 10. Internal API Design (NestJS → Next.js)

| Endpoint | Method | Purpose |
|---|---|---|
| `/api/portfolio/:id` | GET | Current equity, cash, positions |
| `/api/portfolio/:id/performance` | GET | Time series for charting |
| `/api/signals` | GET | Paginated signal history, filterable by symbol/action/date |
| `/api/trades` | GET | Sandbox trade history |
| `/api/strategy-params` | GET/POST | View and update the active parameter set |
| `/api/strategy-params/:id/activate` | POST | Switch active param set (versioned, not destructive) |
| `/api/backtest` | POST | Kick off a backtest run against a date range + param set |
| `/api/backtest/:runId` | GET | Backtest results |

All endpoints behind standard auth (Supabase Auth), even in sandbox phase, to avoid rebuilding this later.

---

## 11. Dashboard (Next.js)

Core views:
1. **Portfolio Overview** — equity curve vs ASI benchmark, current positions, cash balance, today's P&L.
2. **Signal Feed** — live stream of signals (via WebSocket/Redis pub-sub), each showing action, confidence, rationale, and whether it was executed or blocked (and why).
3. **Trade History** — sandbox fills, fees, slippage applied, realized P&L per closed position.
4. **Strategy Configuration** — form-driven editor for `strategy_param_sets`, with version history (never overwrite in place — create a new version and activate it).
5. **Backtest Runner** — pick a date range and param set, view results against live-sandbox performance for comparison.

---

## 12. Scheduling & Job Orchestration

- BullMQ queues on Redis: `market-data-ingest`, `signal-generation`, `sandbox-execution`, `daily-snapshot`, `historical-backfill`.
- Repeatable jobs configured per the §5.2 budget table (`/stocks` every 30 min, `/market` + `/indices` every 60 min, post-close reconciliation once). The local trading-calendar check (§5.2) gates every ingest tick — no wasted API spend outside market hours.
- `historical-backfill` job runs only on non-trading days (weekends/holidays, per the local calendar), pulling 3–5 symbols per run per §5.4, and is skipped entirely on trading days so it never competes with the live-polling budget.
- `daily-snapshot` job runs once after market close to compute `daily_performance_snapshot`.
- Every job that calls NGX Pulse draws from the same Redis token-bucket (§5.6) — if the daily budget is exhausted (e.g. from retries), remaining scheduled jobs for the day back off gracefully rather than erroring, and this is surfaced on the dashboard (§5.6's usage log) rather than failing silently.
- Dead-letter handling: failed jobs (e.g. NGX Pulse timeout) retried with backoff **from the reserved retry pool (§5.2)**, not the scheduled-polling pool, so a burst of transient failures can't cannibalize the day's core price data.

---

## 13. Observability & Audit

- Every signal, every risk-policy decision, and every sandbox trade is persisted — nothing is autonomous-and-untracked. This is the core audit requirement given no human is in the loop for execution.
- Log the exact LLM prompt + raw response alongside each `signals` row (or in a linked `signal_llm_logs` table if payloads get large) so decisions are reproducible and debuggable after the fact.
- Structured logging (e.g. NestJS Logger + a log aggregator) for ingestion failures, rate-limit hits, and risk-policy blocks.

---

## 14. Security & Configuration

- NGX Pulse API key and OpenAI API key stored as environment secrets, never in the repo; rotate-able without redeploy if using a secrets manager.
- Supabase Row Level Security enabled even though this is single-portfolio for now, to avoid retrofitting auth boundaries later.
- Rate-limit guard (§5.3) protects against accidental overuse burning through the NGX Pulse quota.

---

## 15. Non-Functional Requirements

| Concern | Target |
|---|---|
| Data freshness | Price data no more than ~30 min stale during market hours (matches NGX Pulse refresh cadence) |
| Signal latency | Signal generated within 1–2 min of relevant data landing in Supabase |
| Availability | Best-effort for sandbox phase; no uptime SLA needed yet |
| Auditability | 100% of signals and trades persisted with full context — no exceptions |
| Extensibility | Execution layer isolated behind an interface so a real broker adapter can be substituted later without touching signal/risk logic |

---

## 16. Build Phases

1. **Phase 1 — Data foundation**: NGX Pulse integration, ingestion service, Supabase schema, Redis caching, market-status gating.
2. **Phase 2 — Signal engine**: technical indicator computation, Langchain prompt + structured output, signal persistence, dashboard signal feed (read-only).
3. **Phase 3 — Risk policy + sandbox execution**: strategy param sets, deterministic risk checks, fill simulation, portfolio ledger.
4. **Phase 4 — Autonomous loop + scheduling**: BullMQ orchestration end-to-end, daily snapshots, circuit breaker behavior under real market data.
5. **Phase 5 — Backtesting**: historical replay engine, performance metrics, comparison tooling.
6. **Phase 6 — Dashboard polish**: full Next.js UI across portfolio, signals, trades, strategy config, backtest runner.
7. **Phase 7 (future, out of current scope)**: real broker/execution adapter + reintroduction of human-approval gate for live capital.

---

## 17. Open Questions

- If the sandbox strategy proves out, at what point does it make sense to upgrade off Personal tier to unlock fundamentals/disclosures/history and a larger rate-limit headroom?
- Starting mock capital amount and number of concurrent strategy param sets to run in parallel (single sandbox vs. A/B strategy comparison)?
- Desired LLM model and cost ceiling for signal generation frequency × symbol count?
- Should SELL signals be exempt from the daily circuit breaker (to allow risk reduction even after a drawdown halt)?
