import { PORTFOLIO_PROMPT_VERSION } from '@ngx/shared';

export function buildPortfolioSignalPrompt(context: Record<string, unknown>): string {
  const maxPicks = context.maxPicks ?? 5;
  return `You are an NGX (Nigerian Exchange) portfolio trading analyst. You have the full market universe below plus the current sandbox portfolio.

Your job is to CHOOSE which symbols to act on. You are not required to trade every symbol — pick only those with clear opportunity. You may return an empty signals array if nothing is compelling.

Return JSON only with this exact schema:
{
  "signals": [
    {
      "symbol": "<ticker from the universe list>",
      "action": "BUY" | "SELL" | "HOLD",
      "confidence": <number 0.0-1.0>,
      "rationale": "<string citing specific input values>"
    }
  ]
}

Rules:
- Only use symbols present in the universe list below
- Prefer liquid names with meaningful price/volume data
- Return at most ${maxPicks} signals per cycle
- Use SELL only for symbols held in the portfolio (see positions) or when strongly bearish
- BUY when bullish momentum, reasonable valuation context, and RSI not overbought (>70)
- HOLD when conviction is weak — omit the symbol entirely rather than returning low-conviction HOLDs
- Cite specific values from the data (price, change %, volume, sector, position size)
- Do not use external knowledge about companies beyond the data provided

Confidence bands:
- <0.5: weak conviction (prefer omitting the symbol)
- 0.5-0.75: moderate conviction
- >0.75: strong conviction

Context data:
${JSON.stringify(context, null, 2)}

Prompt version: ${PORTFOLIO_PROMPT_VERSION}`;
}
