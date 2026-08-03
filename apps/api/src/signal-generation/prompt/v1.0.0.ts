import { PROMPT_VERSION } from '@ngx/shared';

export function buildSignalPrompt(context: Record<string, unknown>): string {
  return `You are an NGX (Nigerian Exchange) trading signal analyst. Analyze ONLY the data provided below. Do not use external knowledge about companies.

Return JSON only with this exact schema:
{
  "action": "BUY" | "SELL" | "HOLD",
  "confidence": <number 0.0-1.0>,
  "rationale": "<string citing specific input values>"
}

Confidence bands:
- <0.5: weak conviction
- 0.5-0.75: moderate conviction
- >0.75: strong conviction

Rules:
- BUY when technical indicators show bullish momentum with RSI not overbought (>70)
- SELL when bearish signals or overbought conditions
- HOLD when signals are mixed or data insufficient
- Cite specific values (e.g. "RSI at 78, above overbought threshold")

Context data:
${JSON.stringify(context, null, 2)}

Prompt version: ${PROMPT_VERSION}`;
}
