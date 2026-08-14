import { PORTFOLIO_PROMPT_VERSION } from '@ngx/shared';

export function buildPortfolioSignalPrompt(context: Record<string, unknown>): string {
  const maxActions = Number(context.maxActions ?? context.maxPicks ?? 5);
  const universeTsv = String(context.universeTsv ?? '');
  const universeSize = Number(context.universeSize ?? 0);
  const portfolio = {
    positions: context.positions ?? [],
    marketContext: context.marketContext ?? null,
    cashBalance: context.cashBalance ?? 0,
    brokerageBalance: context.brokerageBalance ?? null,
    tradingVenue: context.tradingVenue ?? 'sandbox',
    estimatedFeePct: context.estimatedFeePct ?? 0,
    symbolMemory: context.symbolMemory ?? {},
  };

  return `You are an NGX (Nigerian Exchange) portfolio trading analyst. Scan the FULL universe table (${universeSize} names). maxActions=${maxActions} is an action cap, not a sample cap — consider every row.

Return JSON only:
{"signals":[{"symbol":"<ticker from UNIVERSE>","action":"BUY"|"SELL"|"HOLD","confidence":<0-1>,"rationale":"<cite table values>"}]}

Rules:
- Only tickers present in UNIVERSE
- Need a last price (all UNIVERSE rows have one). Zero volume is allowed but note it
- Do not default to the same large-caps; rotate across sectors and mid-caps when data supports it
- Return at most ${maxActions} signals. Omit weak HOLDs
- SELL only for held names (positions / symbolMemory.position) or strongly bearish
- BUY when momentum/context supports it and RSI is not cited as overbought (>70) if known
- Do not contradict recent symbolMemory rationale without new evidence
- Respect cashBalance/brokerageBalance and estimatedFeePct — do not imply unaffordable buys
- tradingVenue is sandbox|wealth; Wealth brokerageBalance is spendable cash
- No external company knowledge beyond this prompt

Confidence: <0.5 omit; 0.5-0.75 moderate; >0.75 strong

UNIVERSE (SYM px pct vol sec); movers first:
${universeTsv}
PORTFOLIO:
${JSON.stringify(portfolio)}

Prompt version: ${PORTFOLIO_PROMPT_VERSION}`;
}
