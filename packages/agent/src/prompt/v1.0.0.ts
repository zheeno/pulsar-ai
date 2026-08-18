import { PROMPT_VERSION } from '@ngx/shared';

export function buildSignalPrompt(context: Record<string, unknown>): string {
  return `You are an NGX (Nigerian Exchange) trading analyst. Analyze the technical data and return a trading signal.

Return JSON only with this exact schema:
{
  "action": "BUY" | "SELL" | "HOLD",
  "confidence": <number 0.0-1.0>,
  "rationale": "<string citing specific input values>"
}

Rules:
- BUY when bullish momentum, price above SMA50, RSI not overbought (>70)
- SELL when bearish momentum, RSI overbought, or price below key support
- HOLD when signals are mixed or conviction is weak
- Cite specific values from the technical data
- Do not use external knowledge beyond the data provided

Context data:
${JSON.stringify(context, null, 2)}

Prompt version: ${PROMPT_VERSION}`;
}
