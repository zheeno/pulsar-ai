'use client';

import { useEffect, useState } from 'react';
import Nav from '@/components/Nav';

const sections = [
  { id: 'introduction', label: 'Introduction' },
  { id: 'disclaimer', label: 'Important Disclaimer' },
  { id: 'how-it-works', label: 'How It Works' },
  { id: 'getting-started', label: 'Getting Started' },
  { id: 'navigation', label: 'Navigation' },
  { id: 'portfolio', label: 'Portfolio Dashboard' },
  { id: 'run-cycle', label: 'Running a Cycle' },
  { id: 'signals', label: 'Signals' },
  { id: 'trades', label: 'Trades' },
  { id: 'strategy', label: 'Strategy Configuration' },
  { id: 'risk-policy', label: 'Risk Policy' },
  { id: 'backtest', label: 'Backtesting' },
  { id: 'automation', label: 'Automated Scheduling' },
  { id: 'market-data', label: 'Market Data & API Limits' },
  { id: 'indicators', label: 'Technical Indicators' },
  { id: 'llm', label: 'AI Signal Generation' },
  { id: 'troubleshooting', label: 'Troubleshooting' },
  { id: 'glossary', label: 'Glossary' },
];

export default function ManualPage() {
  const [active, setActive] = useState('introduction');

  useEffect(() => {
    const observer = new IntersectionObserver(
      (entries) => {
        const visible = entries
          .filter((e) => e.isIntersecting)
          .sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top);
        if (visible[0]) setActive(visible[0].target.id);
      },
      { rootMargin: '-80px 0px -60% 0px', threshold: 0 },
    );
    sections.forEach(({ id }) => {
      const el = document.getElementById(id);
      if (el) observer.observe(el);
    });
    return () => observer.disconnect();
  }, []);

  return (
    <div>
      <Nav />
      <div style={{ display: 'flex', maxWidth: 1200, margin: '0 auto', padding: '24px 24px 80px', gap: 32 }}>
        <aside style={{
          width: 220, flexShrink: 0, position: 'sticky', top: 24, alignSelf: 'flex-start',
          maxHeight: 'calc(100vh - 48px)', overflowY: 'auto',
        }}>
          <div style={{ fontSize: 13, color: '#64748b', marginBottom: 12, fontWeight: 600, letterSpacing: '0.05em', textTransform: 'uppercase' }}>
            User Manual
          </div>
          {sections.map((s) => (
            <a
              key={s.id}
              href={`#${s.id}`}
              style={{
                display: 'block', padding: '6px 0', fontSize: 14, textDecoration: 'none',
                color: active === s.id ? '#60a5fa' : '#94a3b8',
                fontWeight: active === s.id ? 600 : 400,
                borderLeft: active === s.id ? '2px solid #60a5fa' : '2px solid transparent',
                paddingLeft: 10,
              }}
            >
              {s.label}
            </a>
          ))}
        </aside>

        <main style={{ flex: 1, minWidth: 0 }}>
          <h1 style={{ marginTop: 0, fontSize: 32, marginBottom: 8 }}>NGX AI Trading Assistant — User Manual</h1>
          <p style={{ color: '#94a3b8', fontSize: 16, lineHeight: 1.6, marginBottom: 40 }}>
            A complete guide to understanding, operating, and getting the most out of the platform.
          </p>

          <Section id="introduction" title="Introduction">
            <p>
              The <strong>NGX AI Trading Assistant</strong> (also referred to as <em>Pulsar</em> in deployment) is an
              AI-powered trading sandbox for the <strong>Nigerian Exchange (NGX)</strong>. It ingests live and historical
              market data, analyses equities using technical indicators and large-language-model (LLM) reasoning, generates
              trade signals, and executes those signals against a <strong>simulated portfolio</strong> with realistic
              fill simulation, fees, and slippage.
            </p>
            <p>
              The platform is designed to help you explore automated trading strategies on NGX equities <em>before</em> any
              real capital or broker integration is involved. Every trade you see in the dashboard is a mock execution
              recorded in a sandbox ledger — not a live order placed with a stockbroker.
            </p>
            <h3>What the platform does</h3>
            <ul>
              <li>Ingests NGX market data (stock prices, indices, and market overview) from the NGX Pulse API.</li>
              <li>Computes technical indicators (SMA, RSI, momentum, volume anomaly) for each symbol in your strategy universe.</li>
              <li>Uses an AI model to produce structured BUY / SELL / HOLD signals with confidence scores and written rationales.</li>
              <li>Applies a deterministic risk policy to approve or block each signal before execution.</li>
              <li>Simulates order fills with configurable slippage and brokerage fees.</li>
              <li>Maintains a full audit trail of signals, trades, positions, and daily performance snapshots.</li>
              <li>Provides a web dashboard to monitor portfolio performance, review signals, configure strategy parameters, and run backtests.</li>
            </ul>
            <h3>What the platform does not do</h3>
            <ul>
              <li>Place real orders with any broker or dealer.</li>
              <li>Handle real client funds or connect to a live trading account.</li>
              <li>Guarantee profitable trading outcomes — past sandbox performance is not indicative of future results.</li>
              <li>Provide regulated financial advice. All outputs are for research and simulation purposes only.</li>
            </ul>
          </Section>

          <Section id="disclaimer" title="Important Disclaimer">
            <Callout type="warning">
              This is a <strong>sandbox / mock-execution environment</strong>. The starting capital (default ₦10,000,000)
              is simulated. No real money is at risk, and no real trades are placed on the Nigerian Exchange.
            </Callout>
            <p>
              Signals generated by the AI are probabilistic and based on historical data, technical indicators, and model
              reasoning. They should be treated as research outputs, not as buy or sell recommendations. Before any
              future live-trading phase, a human approval gate and additional compliance controls would be required.
            </p>
            <p>
              Market data is sourced from third-party APIs and may be delayed, incomplete, or unavailable. The platform
              continues operating with cached or previously ingested data when live ingestion fails.
            </p>
          </Section>

          <Section id="how-it-works" title="How It Works">
            <p>
              The platform runs an autonomous loop that can operate on a schedule or be triggered manually. Each
              <strong> trading cycle</strong> follows this pipeline:
            </p>
            <ol>
              <li><strong>Market data ingestion</strong> — Fetch current stock prices, market overview, and index values from NGX Pulse. Store them in the database and cache hot prices in Redis.</li>
              <li><strong>Signal generation</strong> — For each symbol in the active strategy&apos;s allowed list, compute technical indicators, gather fundamental and news context, and call the LLM to produce a structured signal (action, confidence, rationale).</li>
              <li><strong>Risk policy evaluation</strong> — Each signal is checked against the active strategy parameter set: confidence threshold, position limits, daily trade caps, drawdown limits, and symbol allowlist.</li>
              <li><strong>Sandbox execution</strong> — Approved signals are filled at the current price with simulated slippage and fees. The portfolio ledger (cash, positions, trades) is updated atomically.</li>
              <li><strong>Daily snapshot</strong> — Portfolio equity, daily P&amp;L, cumulative P&amp;L, drawdown, and benchmark comparison against the ASI index are recorded.</li>
            </ol>
            <pre style={codeBlock}>{`┌─────────────────┐     ┌──────────────────┐     ┌─────────────┐
│  NGX Pulse API  │────▶│  Ingestion       │────▶│  Database   │
└─────────────────┘     └──────────────────┘     └──────┬──────┘
                                                        │
                        ┌──────────────────┐            │
                        │  LLM + Indicators│◀───────────┘
                        └────────┬─────────┘
                                 │ signals
                        ┌────────▼─────────┐     ┌─────────────┐
                        │  Risk Policy     │────▶│  Sandbox    │
                        └──────────────────┘     │  Execution  │
                                                 └──────┬──────┘
                                                        │
                        ┌──────────────────┐            │
                        │  Dashboard (you) │◀───────────┘
                        └──────────────────┘`}</pre>
            <p>
              Behind the scenes, scheduled cron jobs (running in West Africa Time) can trigger ingestion and snapshots
              automatically during market hours. You can also run the full cycle on demand from the Portfolio dashboard.
            </p>
          </Section>

          <Section id="getting-started" title="Getting Started">
            <h3>1. Sign in</h3>
            <p>
              Open the application URL in your browser. You will be redirected to the login page. Enter your email and
              password. The default development credentials are:
            </p>
            <table style={tableStyle}>
              <tbody>
                <tr><td style={tdStyle}><strong>Email</strong></td><td style={tdStyle}><code>admin@ngx.local</code></td></tr>
                <tr><td style={tdStyle}><strong>Password</strong></td><td style={tdStyle}><code>admin123</code></td></tr>
              </tbody>
            </table>
            <p>Change these credentials in production. Your session is stored as a JWT token in the browser&apos;s local storage.</p>

            <h3>2. Land on the Portfolio dashboard</h3>
            <p>After login you are taken to the <strong>Portfolio</strong> page, which is your home base for monitoring sandbox performance.</p>

            <h3>3. Run your first cycle</h3>
            <p>
              Click the green <strong>Run Cycle</strong> button on the Portfolio page. This triggers a full ingest →
              signal → execute → snapshot pipeline. When it completes, you will see an alert summarising how many signals
              were generated and how many trades were executed.
            </p>

            <h3>4. Explore the results</h3>
            <p>
              Visit the <strong>Signals</strong> page to read AI rationales, and the <strong>Trades</strong> page to see
              simulated fills. Return to Portfolio to view updated positions and the equity curve.
            </p>
          </Section>

          <Section id="navigation" title="Navigation">
            <p>The top navigation bar provides access to all major areas of the application:</p>
            <table style={tableStyle}>
              <thead>
                <tr style={{ borderBottom: '1px solid #334155' }}>
                  <th style={thStyle}>Page</th>
                  <th style={thStyle}>Purpose</th>
                </tr>
              </thead>
              <tbody>
                <tr><td style={tdStyle}><strong>Portfolio</strong></td><td style={tdStyle}>Overview of sandbox equity, positions, equity curve, NGX API usage, and manual cycle trigger.</td></tr>
                <tr><td style={tdStyle}><strong>Signals</strong></td><td style={tdStyle}>Feed of all AI-generated trade signals with confidence, rationale, and risk policy outcome.</td></tr>
                <tr><td style={tdStyle}><strong>Trades</strong></td><td style={tdStyle}>History of executed sandbox trades with fill prices, fees, and slippage.</td></tr>
                <tr><td style={tdStyle}><strong>Strategy</strong></td><td style={tdStyle}>Create and activate strategy parameter sets that control risk and position sizing.</td></tr>
                <tr><td style={tdStyle}><strong>Backtest</strong></td><td style={tdStyle}>Run historical simulations of a strategy against stored price data.</td></tr>
                <tr><td style={tdStyle}><strong>Manual</strong></td><td style={tdStyle}>This guide.</td></tr>
              </tbody>
            </table>
            <p>Use <strong>Logout</strong> in the top-right corner to end your session.</p>
          </Section>

          <Section id="portfolio" title="Portfolio Dashboard">
            <p>
              The Portfolio page (<code>/dashboard</code>) is the primary monitoring view. It auto-refreshes every
              30 seconds.
            </p>
            <h3>Summary cards</h3>
            <table style={tableStyle}>
              <thead>
                <tr style={{ borderBottom: '1px solid #334155' }}>
                  <th style={thStyle}>Metric</th>
                  <th style={thStyle}>Description</th>
                </tr>
              </thead>
              <tbody>
                <tr><td style={tdStyle}><strong>Total Equity</strong></td><td style={tdStyle}>Cash balance plus the current market value of all open positions.</td></tr>
                <tr><td style={tdStyle}><strong>Cash Balance</strong></td><td style={tdStyle}>Uninvested mock cash remaining in the sandbox portfolio.</td></tr>
                <tr><td style={tdStyle}><strong>Market Value</strong></td><td style={tdStyle}>Combined current value of all held positions at latest known prices.</td></tr>
                <tr><td style={tdStyle}><strong>Today&apos;s P&amp;L</strong></td><td style={tdStyle}>Change in total equity since the previous daily snapshot. Green = positive, red = negative.</td></tr>
              </tbody>
            </table>

            <h3>NGX Pulse API usage</h3>
            <p>
              A banner shows how many NGX Pulse API requests have been used today out of the daily limit (100 on the
              free tier). Ingestion is rate-limited to stay within this budget. If you see ingestion warnings after
              running a cycle, the platform may have continued using cached price data.
            </p>

            <h3>Equity curve</h3>
            <p>
              A line chart plots <code>total_equity</code> from daily performance snapshots over time. This is the best
              visual indicator of how the sandbox strategy is performing relative to its starting capital of ₦10,000,000.
            </p>

            <h3>Positions table</h3>
            <p>Lists every open position with:</p>
            <ul>
              <li><strong>Symbol</strong> — NGX ticker (e.g. DANGCEM, GTCO).</li>
              <li><strong>Qty</strong> — Number of shares held.</li>
              <li><strong>Avg Cost</strong> — Volume-weighted average purchase price.</li>
              <li><strong>Current</strong> — Latest known market price.</li>
              <li><strong>Value</strong> — Qty × Current price.</li>
            </ul>
          </Section>

          <Section id="run-cycle" title="Running a Cycle">
            <p>
              The <strong>Run Cycle</strong> button on the Portfolio page triggers the full autonomous pipeline manually.
              This is useful for testing strategy changes, forcing a fresh data pull, or running outside scheduled hours.
            </p>
            <h3>What happens when you click Run Cycle</h3>
            <ol>
              <li><strong>Force ingestion</strong> — Stocks, market overview, and indices are fetched from NGX Pulse regardless of whether the market is currently open.</li>
              <li><strong>Signal generation</strong> — The engine iterates over every symbol in the active strategy&apos;s <code>allowed_symbols</code> list and generates one signal per symbol.</li>
              <li><strong>Execution</strong> — Each generated signal is evaluated by the risk policy. Approved signals result in a simulated trade.</li>
              <li><strong>Snapshot</strong> — A daily performance snapshot is created or updated for the portfolio.</li>
            </ol>
            <h3>Completion alert</h3>
            <p>When the cycle finishes, an alert shows:</p>
            <ul>
              <li>Number of signals generated.</li>
              <li>Number of trades executed (signals that passed risk policy).</li>
              <li>A warning if live ingestion was skipped (e.g. invalid API key or rate limit exceeded). The cycle still continues using existing price data.</li>
            </ul>
            <Callout type="info">
              Running cycles frequently consumes NGX Pulse API quota and OpenAI tokens (if configured). On the free NGX
              Pulse tier, you have 100 requests per day and 10 per minute.
            </Callout>
          </Section>

          <Section id="signals" title="Signals">
            <p>
              The Signals page (<code>/signals</code>) displays every decision the AI engine has produced, whether or
              not it was executed.
            </p>
            <h3>Reading a signal card</h3>
            <ul>
              <li><strong>Action</strong> — <span style={{ color: '#22c55e' }}>BUY</span>, <span style={{ color: '#ef4444' }}>SELL</span>, or HOLD.</li>
              <li><strong>Symbol</strong> — The NGX ticker the signal applies to.</li>
              <li><strong>Confidence</strong> — A score from 0–100% representing the model&apos;s conviction. Signals below the strategy&apos;s minimum confidence threshold are blocked from execution.</li>
              <li><strong>Rationale</strong> — A plain-English explanation of why the model chose this action, referencing indicators and context.</li>
              <li><strong>Risk</strong> — The outcome of risk policy evaluation (see Risk Policy section). Shows &quot;✓ Executed&quot; if a sandbox trade was placed.</li>
              <li><strong>Timestamp</strong> — When the signal was generated.</li>
            </ul>
            <h3>Filtering</h3>
            <p>
              Use the symbol text filter and action dropdown (BUY / SELL / HOLD) to narrow the feed. Filters are applied
              immediately on change.
            </p>
            <p>
              If no signals appear, run a cycle from the Portfolio page. Signals are only created for symbols in the
              active strategy&apos;s allowed list that have sufficient price history (at least 60 trading days).
            </p>
          </Section>

          <Section id="trades" title="Trades">
            <p>
              The Trades page (<code>/trades</code>) lists every <strong>executed</strong> sandbox trade — only signals
              that passed risk policy and were filled.
            </p>
            <table style={tableStyle}>
              <thead>
                <tr style={{ borderBottom: '1px solid #334155' }}>
                  <th style={thStyle}>Column</th>
                  <th style={thStyle}>Meaning</th>
                </tr>
              </thead>
              <tbody>
                <tr><td style={tdStyle}><strong>Time</strong></td><td style={tdStyle}>When the simulated fill occurred.</td></tr>
                <tr><td style={tdStyle}><strong>Symbol</strong></td><td style={tdStyle}>NGX ticker traded.</td></tr>
                <tr><td style={tdStyle}><strong>Side</strong></td><td style={tdStyle}>BUY (green) or SELL (red).</td></tr>
                <tr><td style={tdStyle}><strong>Qty</strong></td><td style={tdStyle}>Number of shares filled.</td></tr>
                <tr><td style={tdStyle}><strong>Fill Price</strong></td><td style={tdStyle}>Simulated execution price after slippage adjustment.</td></tr>
                <tr><td style={tdStyle}><strong>Fee</strong></td><td style={tdStyle}>Simulated brokerage fee (default 0.15% of notional).</td></tr>
                <tr><td style={tdStyle}><strong>Slippage</strong></td><td style={tdStyle}>Slippage applied in basis points (default 10 bps = 0.10%).</td></tr>
              </tbody>
            </table>
            <h3>How fills are simulated</h3>
            <p>
              BUY orders fill slightly above the quoted price; SELL orders fill slightly below, reflecting realistic
              market impact. Fees are deducted from cash on buys and subtracted from proceeds on sells. The resulting
              cash balance is stored with each trade record.
            </p>
          </Section>

          <Section id="strategy" title="Strategy Configuration">
            <p>
              The Strategy page (<code>/strategy</code>) lets you define <strong>parameter sets</strong> that control
              how aggressively the system trades and how risk is managed. Only one parameter set can be active at a time.
            </p>
            <h3>Creating a new version</h3>
            <p>Fill in the form and click <strong>Create Version</strong>. New versions are created in an inactive state — you must explicitly activate them.</p>
            <table style={tableStyle}>
              <thead>
                <tr style={{ borderBottom: '1px solid #334155' }}>
                  <th style={thStyle}>Parameter</th>
                  <th style={thStyle}>Description</th>
                  <th style={thStyle}>Default</th>
                </tr>
              </thead>
              <tbody>
                <tr>
                  <td style={tdStyle}><strong>Name</strong></td>
                  <td style={tdStyle}>A human-readable label for this version (e.g. &quot;conservative-v2&quot;).</td>
                  <td style={tdStyle}>—</td>
                </tr>
                <tr>
                  <td style={tdStyle}><strong>Max Position %</strong></td>
                  <td style={tdStyle}>Maximum portfolio exposure to a single symbol. E.g. 0.10 = no more than 10% of total equity in one stock.</td>
                  <td style={tdStyle}>10%</td>
                </tr>
                <tr>
                  <td style={tdStyle}><strong>Max Daily Trades</strong></td>
                  <td style={tdStyle}>Maximum number of BUY trades allowed per calendar day. SELL orders are not counted against this limit.</td>
                  <td style={tdStyle}>5</td>
                </tr>
                <tr>
                  <td style={tdStyle}><strong>Stop Loss %</strong></td>
                  <td style={tdStyle}>Price decline from average cost that triggers stop-loss logic on SELL evaluation.</td>
                  <td style={tdStyle}>5%</td>
                </tr>
                <tr>
                  <td style={tdStyle}><strong>Min Confidence</strong></td>
                  <td style={tdStyle}>Minimum AI confidence (0–1) required for a signal to be eligible for execution. E.g. 0.65 = 65%.</td>
                  <td style={tdStyle}>0.65</td>
                </tr>
                <tr>
                  <td style={tdStyle}><strong>Max Daily Drawdown %</strong></td>
                  <td style={tdStyle}>If the portfolio&apos;s drawdown for the day exceeds this threshold, new BUY orders are blocked.</td>
                  <td style={tdStyle}>3%</td>
                </tr>
                <tr>
                  <td style={tdStyle}><strong>Position Size %</strong></td>
                  <td style={tdStyle}>Target allocation per new BUY as a fraction of total equity. E.g. 0.05 = 5% of equity per trade.</td>
                  <td style={tdStyle}>5%</td>
                </tr>
              </tbody>
            </table>
            <h3>Activating a version</h3>
            <p>
              In the <strong>Version History</strong> list, click <strong>Activate</strong> on any inactive version.
              This deactivates the current active set and applies the new parameters to all future cycles and signal
              generation. The active version is marked with a green <strong>ACTIVE</strong> badge.
            </p>
            <Callout type="info">
              The seeded default strategy includes a curated list of NGX symbols (e.g. DANGCEM, GTCO, ZENITHBANK, MTNN).
              Symbol allowlists are configured in the database seed, not in the web UI. New UI-created versions inherit
              null allowlists until configured server-side.
            </Callout>
          </Section>

          <Section id="risk-policy" title="Risk Policy">
            <p>
              After the AI generates a signal, a <strong>deterministic risk policy engine</strong> decides whether it
              may proceed to execution. This layer is separate from the LLM — it enforces hard rules that cannot be
              overridden by model output.
            </p>
            <h3>Risk policy outcomes</h3>
            <table style={tableStyle}>
              <thead>
                <tr style={{ borderBottom: '1px solid #334155' }}>
                  <th style={thStyle}>Result</th>
                  <th style={thStyle}>Meaning</th>
                </tr>
              </thead>
              <tbody>
                <tr><td style={tdStyle}><code>APPROVED</code></td><td style={tdStyle}>Signal passed all checks. A sandbox trade will be placed.</td></tr>
                <tr><td style={tdStyle}><code>BLOCKED_CONFIDENCE</code></td><td style={tdStyle}>AI confidence is below the strategy&apos;s minimum threshold.</td></tr>
                <tr><td style={tdStyle}><code>BLOCKED_SYMBOL</code></td><td style={tdStyle}>Symbol is not in the strategy&apos;s allowed list.</td></tr>
                <tr><td style={tdStyle}><code>BLOCKED_DAILY_TRADES</code></td><td style={tdStyle}>Maximum daily BUY count has been reached.</td></tr>
                <tr><td style={tdStyle}><code>BLOCKED_DRAWDOWN</code></td><td style={tdStyle}>Portfolio daily drawdown exceeds the configured limit.</td></tr>
                <tr><td style={tdStyle}><code>BLOCKED_EXPOSURE</code></td><td style={tdStyle}>Trade would push single-name exposure above the max position percentage, or computed quantity is zero.</td></tr>
                <tr><td style={tdStyle}><code>BLOCKED_OTHER</code></td><td style={tdStyle}>Catch-all: HOLD signals, SELL with no position, missing price data, etc.</td></tr>
              </tbody>
            </table>
            <h3>Position sizing logic</h3>
            <p>For approved BUY signals, quantity is calculated as:</p>
            <ol>
              <li>Compute target value = total equity × position size %.</li>
              <li>Compute max additional value = (total equity × max position %) − current position value.</li>
              <li>Allocate the lesser of target and max additional value.</li>
              <li>Divide by current price and floor to whole shares.</li>
            </ol>
            <p>For SELL signals, the engine sells the entire existing position if approved.</p>
          </Section>

          <Section id="backtest" title="Backtesting">
            <p>
              The Backtest page (<code>/backtest</code>) lets you simulate how a strategy parameter set would have
              performed over a historical date range using stored price data.
            </p>
            <h3>Running a backtest</h3>
            <ol>
              <li>Select a <strong>Strategy Param Set</strong> from the dropdown (active sets are labelled).</li>
              <li>Set a <strong>Start Date</strong> and <strong>End Date</strong> within the range of available price history.</li>
              <li>Click <strong>Start Backtest</strong>. The run executes asynchronously on the server.</li>
              <li>The page polls every 2 seconds until the run status is <code>completed</code> or <code>failed</code>.</li>
            </ol>
            <h3>Results</h3>
            <table style={tableStyle}>
              <thead>
                <tr style={{ borderBottom: '1px solid #334155' }}>
                  <th style={thStyle}>Metric</th>
                  <th style={thStyle}>Description</th>
                </tr>
              </thead>
              <tbody>
                <tr><td style={tdStyle}><strong>Total Return</strong></td><td style={tdStyle}>Percentage gain or loss from the ₦10M starting capital over the backtest period.</td></tr>
                <tr><td style={tdStyle}><strong>Max Drawdown</strong></td><td style={tdStyle}>Largest peak-to-trough decline in equity during the period.</td></tr>
                <tr><td style={tdStyle}><strong>Win Rate</strong></td><td style={tdStyle}>Percentage of SELL trades that closed at a profit.</td></tr>
                <tr><td style={tdStyle}><strong>Trades</strong></td><td style={tdStyle}>Total number of simulated round-trip or entry trades.</td></tr>
              </tbody>
            </table>
            <p>An equity curve chart shows portfolio value over each trading day in the range.</p>
            <Callout type="info">
              Backtests use simplified technical indicators when full history is unavailable and may reuse cached LLM
              signals from prior live runs. Results are indicative, not predictive. Always validate strategy changes
              with multiple date ranges.
            </Callout>
          </Section>

          <Section id="automation" title="Automated Scheduling">
            <p>
              In addition to manual cycles, the backend orchestrator runs scheduled jobs automatically (West Africa
              Time, UTC+1). You do not need to keep the browser open for these to run.
            </p>
            <table style={tableStyle}>
              <thead>
                <tr style={{ borderBottom: '1px solid #334155' }}>
                  <th style={thStyle}>Schedule</th>
                  <th style={thStyle}>Job</th>
                </tr>
              </thead>
              <tbody>
                <tr><td style={tdStyle}>Every 30 min, 9:00–15:30, Mon–Fri</td><td style={tdStyle}>Ingest stock prices (only when market is open).</td></tr>
                <tr><td style={tdStyle}>Hourly 10:00–15:00, Mon–Fri</td><td style={tdStyle}>Ingest market overview and index values.</td></tr>
                <tr><td style={tdStyle}>16:05, Mon–Fri</td><td style={tdStyle}>Post-close stock reconciliation and daily performance snapshot.</td></tr>
                <tr><td style={tdStyle}>10:00, Sat–Sun</td><td style={tdStyle}>Weekend historical price backfill for symbols missing data.</td></tr>
              </tbody>
            </table>
            <p>
              NGX trading hours are approximately <strong>10:00 AM – 2:30 PM WAT</strong> on business days, excluding
              public holidays. The platform respects the NGX holiday calendar and skips ingestion on non-trading days
              unless you force a manual cycle.
            </p>
          </Section>

          <Section id="market-data" title="Market Data & API Limits">
            <h3>Data source</h3>
            <p>
              Live market data comes from the <strong>NGX Pulse API</strong> (<code>ngxpulse.ng</code>). The platform
              ingests stock snapshots, market overview (including ASI), and index values. Historical prices are stored
              for backtesting and indicator computation.
            </p>
            <h3>Rate limits (free tier)</h3>
            <table style={tableStyle}>
              <tbody>
                <tr><td style={tdStyle}><strong>Daily limit</strong></td><td style={tdStyle}>100 requests per day</td></tr>
                <tr><td style={tdStyle}><strong>Per-minute limit</strong></td><td style={tdStyle}>10 requests per minute</td></tr>
              </tbody>
            </table>
            <p>
              The Portfolio page shows your current daily usage. If the API key is missing or invalid, the platform
              falls back to <strong>mock data</strong> for development and logs a warning. In production, set a valid
              <code>NGX_PULSE_API_KEY</code> in the server environment.
            </p>
            <h3>Price caching</h3>
            <p>
              Latest prices are cached in Redis for fast access during execution and portfolio valuation. If Redis
              cache is empty, the system falls back to the most recent row in <code>price_history</code>.
            </p>
          </Section>

          <Section id="indicators" title="Technical Indicators">
            <p>Before calling the LLM, the signal engine computes these indicators from up to 250 days of price history:</p>
            <table style={tableStyle}>
              <thead>
                <tr style={{ borderBottom: '1px solid #334155' }}>
                  <th style={thStyle}>Indicator</th>
                  <th style={thStyle}>Description</th>
                </tr>
              </thead>
              <tbody>
                <tr><td style={tdStyle}><strong>SMA 50</strong></td><td style={tdStyle}>50-day simple moving average — medium-term trend.</td></tr>
                <tr><td style={tdStyle}><strong>SMA 200</strong></td><td style={tdStyle}>200-day simple moving average — long-term trend.</td></tr>
                <tr><td style={tdStyle}><strong>RSI 14</strong></td><td style={tdStyle}>14-period Relative Strength Index. Below 30 = oversold; above 70 = overbought.</td></tr>
                <tr><td style={tdStyle}><strong>Momentum (20-day)</strong></td><td style={tdStyle}>Percentage price change over the last 20 trading days.</td></tr>
                <tr><td style={tdStyle}><strong>Volume Anomaly</strong></td><td style={tdStyle}>Ratio of today&apos;s volume to the 20-day average. Values above 1 indicate unusually high activity.</td></tr>
              </tbody>
            </table>
            <p>
              At least <strong>60 days</strong> of price history is required before a symbol can receive a signal.
              Indicators and the current price are stored in each signal&apos;s <code>technical_snapshot</code> for auditability.
            </p>
          </Section>

          <Section id="llm" title="AI Signal Generation">
            <p>
              The signal engine uses a large language model (default: <strong>gpt-4o-mini</strong> via OpenAI) to
              synthesise technical, fundamental, and news context into a structured trading decision.
            </p>
            <h3>Input context per symbol</h3>
            <ul>
              <li>Computed technical indicators and current price.</li>
              <li>Latest fundamentals snapshot (if available).</li>
              <li>Recent news headlines (last 48 hours).</li>
              <li>ASI index value and weekly change for market context.</li>
            </ul>
            <h3>Output format</h3>
            <p>The model returns a JSON object with:</p>
            <ul>
              <li><code>action</code> — BUY, SELL, or HOLD.</li>
              <li><code>confidence</code> — Float from 0.0 to 1.0.</li>
              <li><code>rationale</code> — Human-readable explanation.</li>
            </ul>
            <p>
              The full prompt and raw model response are stored in the database for every signal, enabling post-hoc
              review and prompt iteration.
            </p>
            <h3>Mock mode</h3>
            <p>
              If <code>OPENAI_API_KEY</code> is not configured, the platform uses a rule-based mock that reacts to RSI
              and momentum values. This is useful for development but produces less nuanced signals than a live model.
            </p>
          </Section>

          <Section id="troubleshooting" title="Troubleshooting">
            <h3>Run Cycle returns 0 ingested / shows a warning</h3>
            <p>
              Live NGX ingestion failed. Common causes: invalid or missing <code>NGX_PULSE_API_KEY</code>, daily rate
              limit exceeded, or NGX API downtime. The cycle still runs using cached prices. Check API usage on the
              Portfolio page and verify server environment variables.
            </p>
            <h3>No signals after a cycle</h3>
            <ul>
              <li>Ensure an active strategy parameter set exists (Strategy page → look for ACTIVE badge).</li>
              <li>Symbols need at least 60 days of price history. Run ingestion over multiple days or use weekend backfill.</li>
              <li>The AI may output HOLD for all symbols if indicators are mixed — this is expected behaviour.</li>
            </ul>
            <h3>Signals generated but 0 executed</h3>
            <p>
              Signals were blocked by risk policy. Check the Risk column on the Signals page. Common blocks: confidence
              too low, daily trade limit reached, or max exposure exceeded.
            </p>
            <h3>Backtest stuck on &quot;running&quot;</h3>
            <p>
              Large date ranges with many symbols take longer. If it fails, check server logs. Ensure price history
              exists for the selected date range.
            </p>
            <h3>Logged out unexpectedly</h3>
            <p>
              JWT tokens expire or become invalid. The app redirects to login on any 401 response. Sign in again to
              continue.
            </p>
            <h3>Equity curve is empty</h3>
            <p>
              Daily snapshots are created after each cycle or at post-close (16:05 WAT). Run at least one cycle or wait
              for the scheduled snapshot job.
            </p>
          </Section>

          <Section id="glossary" title="Glossary">
            <table style={tableStyle}>
              <tbody>
                <tr><td style={tdStyle}><strong>ASI</strong></td><td style={tdStyle}>NGX All-Share Index — the primary benchmark for Nigerian equities.</td></tr>
                <tr><td style={tdStyle}><strong>bps</strong></td><td style={tdStyle}>Basis points. 1 bps = 0.01%. Used for slippage measurement.</td></tr>
                <tr><td style={tdStyle}><strong>Cycle</strong></td><td style={tdStyle}>One full ingest → signal → execute → snapshot pipeline run.</td></tr>
                <tr><td style={tdStyle}><strong>Drawdown</strong></td><td style={tdStyle}>Decline from a portfolio equity peak, expressed as a percentage.</td></tr>
                <tr><td style={tdStyle}><strong>LLM</strong></td><td style={tdStyle}>Large Language Model — the AI used for signal rationale and action selection.</td></tr>
                <tr><td style={tdStyle}><strong>NGX</strong></td><td style={tdStyle}>Nigerian Exchange — the stock exchange for equities traded in Naira.</td></tr>
                <tr><td style={tdStyle}><strong>Notional</strong></td><td style={tdStyle}>Total value of a trade = fill price × quantity.</td></tr>
                <tr><td style={tdStyle}><strong>Param set</strong></td><td style={tdStyle}>A versioned collection of strategy risk and sizing parameters.</td></tr>
                <tr><td style={tdStyle}><strong>Sandbox</strong></td><td style={tdStyle}>The simulated trading environment — no real money or broker orders.</td></tr>
                <tr><td style={tdStyle}><strong>Signal</strong></td><td style={tdStyle}>An AI-generated recommendation (BUY/SELL/HOLD) for a specific symbol.</td></tr>
                <tr><td style={tdStyle}><strong>Slippage</strong></td><td style={tdStyle}>Difference between expected and simulated fill price due to market impact.</td></tr>
                <tr><td style={tdStyle}><strong>WAT</strong></td><td style={tdStyle}>West Africa Time (UTC+1) — timezone used for market hours and scheduling.</td></tr>
              </tbody>
            </table>
          </Section>
        </main>
      </div>
    </div>
  );
}

function Section({ id, title, children }: { id: string; title: string; children: React.ReactNode }) {
  return (
    <section id={id} style={{ marginBottom: 48, scrollMarginTop: 24 }}>
      <h2 style={{ fontSize: 22, marginBottom: 16, paddingBottom: 8, borderBottom: '1px solid #334155' }}>{title}</h2>
      <div style={{ lineHeight: 1.7, color: '#cbd5e1', fontSize: 15 }}>{children}</div>
    </section>
  );
}

function Callout({ type, children }: { type: 'info' | 'warning'; children: React.ReactNode }) {
  const colors = type === 'warning'
    ? { bg: '#422006', border: '#92400e', text: '#fde68a' }
    : { bg: '#1e3a5f', border: '#1d4ed8', text: '#bfdbfe' };
  return (
    <div style={{
      background: colors.bg, border: `1px solid ${colors.border}`, borderRadius: 8,
      padding: '14px 18px', margin: '16px 0', color: colors.text, fontSize: 14, lineHeight: 1.6,
    }}>
      {children}
    </div>
  );
}

const tableStyle: React.CSSProperties = {
  width: '100%', borderCollapse: 'collapse', margin: '12px 0 20px', fontSize: 14,
};

const thStyle: React.CSSProperties = {
  padding: '10px 12px', textAlign: 'left', color: '#94a3b8', fontWeight: 600,
};

const tdStyle: React.CSSProperties = {
  padding: '10px 12px', borderTop: '1px solid #1e293b', verticalAlign: 'top',
};

const codeBlock: React.CSSProperties = {
  background: '#0f172a', border: '1px solid #334155', borderRadius: 8,
  padding: 16, overflowX: 'auto', fontSize: 12, lineHeight: 1.5, color: '#94a3b8',
};
