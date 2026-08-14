import { PORTFOLIO_PROMPT_VERSION } from '@ngx/shared';

export function buildPortfolioSignalPrompt(context: Record<string, unknown>): string {
  const maxActions = Number(context.maxActions ?? context.maxPicks ?? 5);
  const universeTsv = String(context.universeTsv ?? '');
  const universeSize = Number(context.universeSize ?? 0);
  const held = context.held ?? [];
  const strategy = context.strategy ?? {};
  const portfolio = {
    positions: context.positions ?? [],
    held,
    strategy,
    marketContext: context.marketContext ?? null,
    cashBalance: context.cashBalance ?? 0,
    brokerageBalance: context.brokerageBalance ?? null,
    tradingVenue: context.tradingVenue ?? 'sandbox',
    estimatedFeePct: context.estimatedFeePct ?? 0,
    symbolMemory: context.symbolMemory ?? {},
  };

  return `You are an NGX (Nigerian Exchange) portfolio trading analyst. Scan HELD names first, then the FULL universe table (${universeSize} names). maxActions=${maxActions} is an action cap for YOUR output (rule-based stop-loss / take-profit sells are already queued separately).

Return JSON only:
{"signals":[{"symbol":"<ticker from UNIVERSE>","action":"BUY"|"SELL"|"HOLD","confidence":<0-1>,"rationale":"<cite table values>"}]}

Rules:
- Only tickers present in UNIVERSE
- Need a last price (all UNIVERSE rows have one). Zero volume is allowed but note it
- Review HELD first. Do not SELL names whose exitHint is stop_loss or take_profit (those exits are already generated)
- SELL only for held names (HELD / positions / symbolMemory.position) that have not already been flagged
- Discretionary SELL of a held name is allowed when thesis is broken even if exitHint is hold
- Remaining slots: new BUYs across sectors and mid-caps when data supports it; do not default to the same large-caps
- Return at most ${maxActions} signals. Omit weak HOLDs
- BUY when momentum/context supports it and RSI is not cited as overbought (>70) if known
- Do not contradict recent symbolMemory rationale without new evidence
- Respect cashBalance/brokerageBalance and estimatedFeePct — do not imply unaffordable buys
- tradingVenue is sandbox|wealth; Wealth brokerageBalance is spendable cash
- Stop loss / take profit thresholds are in strategy; do not re-fire those rule exits
- No external company knowledge beyond this prompt

Confidence: <0.5 omit; 0.5-0.75 moderate; >0.75 strong

UNIVERSE (SYM px pct vol sec); held names first, then movers:
${universeTsv}
PORTFOLIO:
${JSON.stringify(portfolio)}

Prompt version: ${PORTFOLIO_PROMPT_VERSION}`;
}
