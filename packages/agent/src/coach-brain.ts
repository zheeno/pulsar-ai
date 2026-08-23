import {
  CURATED_SYMBOLS,
  LlmStrategyCoachOutputSchema,
  STRATEGY_COACH_PROMPT_VERSION,
  type LlmStrategyCoachOutput,
} from '@ngx/shared';

export type CoachIntentClass =
  | 'meta'
  | 'greeting'
  | 'research'
  | 'account'
  | 'strategy'
  | 'trade'
  | 'other';

export type ChatTurn = { role: string; content: string };

export const PERSONA_META =
  "I'm Coach, Pulsar's desk copilot for NGX equities and Busha crypto. I can check the tape, a name's history, crypto headlines from CoinDesk / Decrypt / The Block RSS, help you tighten risk sliders, and propose trades. NGX news is not wired — I won't invent it. I won't silently place live orders or invent prices. What do you want to look at?";

export const PERSONA_GREETING =
  "Hey. Tape, a ticker, news, risk sliders, or a trade idea — your call.";

export const PERSONA_THANKS = 'Anytime. Ping me if you want the tape, a name, or a trade idea.';

export const PERSONA_BYE = "See you. I'll be here when you want the next look.";

const META_PHRASES = [
  'tell me about yourself',
  'tell me about you',
  'about yourself',
  'who are you',
  "who're you",
  'who are u',
  'what are you',
  'what can you do',
  'what do you do',
  'how do you work',
  'how does this work',
  'how does coach work',
  'your capabilities',
  'introduce yourself',
  'what is coach',
  "what's your purpose",
  'whats your purpose',
  'what is your purpose',
];

const GREETING_EXACT = new Set([
  'hi',
  'hey',
  'hello',
  'yo',
  'sup',
  'hiya',
  'howdy',
  'gm',
  'good morning',
  'good afternoon',
  'good evening',
  'thanks',
  'thank you',
  'thanks!',
  'cheers',
  'ty',
  'bye',
  'goodbye',
  'good bye',
  'later',
  'see ya',
  'see you',
  'ok thanks',
  'okay thanks',
]);

const RESEARCH_KEYS = [
  'price',
  'quote',
  'moving',
  'movers',
  'gainer',
  'loser',
  'news',
  'headline',
  'history',
  'how is',
  "how's",
  'rsi',
  'sma',
  'indicator',
  'chart',
  'tape',
  'universe',
  'advisable',
  'should i buy',
  'should i sell',
  'worth buying',
  'thoughts on',
];

const STRATEGY_KEYS = [
  'tighten',
  'loosen',
  'stop loss',
  'stop-loss',
  'take profit',
  'take-profit',
  'drawdown',
  'slider',
  'cycle budget',
  'min confidence',
  'risk settings',
  'you decide',
  'your call',
  'you choose',
  'you pick',
  'you should decide',
];

const ACCOUNT_KEYS = [
  'cash',
  'balance',
  'holdings',
  'holding',
  'positions',
  'position',
  'portfolio',
  'pnl',
  'p&l',
  'my book',
  'how much do i',
  'how much have i',
  'what do i hold',
  'open lots',
];

const COACH_AGENT_TOOLS = [
  'get_account_snapshot',
  'get_holdings',
  'get_recent_trades',
  'get_strategy_params',
  'list_universe_quotes',
  'get_symbol_quote',
  'get_price_history',
  'get_indicators',
  'search_memory',
  'memory_search',
  'get_news',
  'run_symbol_screen',
  'explain_blocked_reason',
  'get_cycle_status',
  'get_trade_lessons',
  'get_dream_rules',
  'get_last_cycle',
  'get_confidence_journal',
  'propose_strategy_patch',
  'propose_trade',
];

const IRREVERSIBLE_TOOLS = new Set(['execute_trade', 'apply_strategy_patch']);

export function parseCoachOutput(raw: string): LlmStrategyCoachOutput {
  const trimmed = raw.trim();
  if (!trimmed) {
    return LlmStrategyCoachOutputSchema.parse({
      summary: PERSONA_GREETING,
      patch: {},
    });
  }

  const candidates: string[] = [trimmed];
  const fenced = trimmed.match(/```(?:json)?\s*([\s\S]*?)```/i);
  if (fenced?.[1]?.trim()) candidates.push(fenced[1].trim());

  for (const candidate of candidates) {
    const jsonMatch = candidate.match(/\{[\s\S]*\}/);
    if (!jsonMatch) continue;
    try {
      return LlmStrategyCoachOutputSchema.parse(JSON.parse(jsonMatch[0]));
    } catch {
      // try next candidate
    }
  }

  // Model replied in plain language — wrap as the chat bubble (conversation-first).
  return LlmStrategyCoachOutputSchema.parse({
    needMoreContext: false,
    clarifyingQuestions: [],
    summary: trimmed,
    patch: {},
    rationale: {},
    warnings: [],
  });
}

export function classifyCoachIntent(message: string): CoachIntentClass {
  const t = message.trim().toLowerCase();
  if (!t) return 'other';
  if (looksLikeMeta(t)) return 'meta';
  if (looksLikeGreeting(t)) return 'greeting';
  if (looksLikeTrade(t) && !looksLikeAdvisory(t)) return 'trade';
  if (looksLikeStrategy(t)) return 'strategy';
  if (looksLikeAccount(t)) return 'account';
  if (looksLikeResearch(t)) return 'research';
  return 'other';
}

export function resolveCoachIntent(
  message: string,
  fromContext?: string,
): CoachIntentClass {
  const classes: CoachIntentClass[] = [
    'meta',
    'greeting',
    'research',
    'account',
    'strategy',
    'trade',
    'other',
  ];
  if (fromContext && classes.includes(fromContext as CoachIntentClass)) {
    return fromContext as CoachIntentClass;
  }
  return classifyCoachIntent(message);
}

export function allowedTools(_intent?: CoachIntentClass): string[] {
  return [...COACH_AGENT_TOOLS];
}

export function isIrreversibleCoachTool(name: string): boolean {
  return IRREVERSIBLE_TOOLS.has(name);
}

export function toolAllowed(_intent: CoachIntentClass, name: string): boolean {
  return !isIrreversibleCoachTool(name);
}

export function attachesBook(_intent?: CoachIntentClass): boolean {
  return false;
}

export function cannedSocialReply(intent: CoachIntentClass, message: string): string {
  const t = message.trim().toLowerCase();
  if (intent === 'meta') return PERSONA_META;
  if (/\b(thanks|thank you|cheers|ty)\b/.test(t)) return PERSONA_THANKS;
  if (/\b(bye|goodbye|later|see ya|see you)\b/.test(t)) return PERSONA_BYE;
  return PERSONA_GREETING;
}

export async function gateToolCall(
  _intent: CoachIntentClass,
  name: string,
  args: Record<string, unknown>,
  callTool: (name: string, args: Record<string, unknown>) => Promise<unknown>,
): Promise<unknown> {
  if (isIrreversibleCoachTool(name)) {
    return {
      ok: false,
      refused: true,
      error: `${name} must be confirmed in the UI, not called from chat`,
    };
  }
  return callTool(name, args);
}

export function buildCoachMessages(context: Record<string, unknown>): {
  role: 'system' | 'user' | 'assistant';
  content: string;
}[] {
  const conv = (context.conversation ?? {}) as {
    message?: string;
    history?: ChatTurn[];
  };
  const history = Array.isArray(conv.history) ? conv.history : [];
  const message = String(conv.message ?? '').trim();
  const out: { role: 'system' | 'user' | 'assistant'; content: string }[] = [
    { role: 'system', content: buildCoachSystemPrompt(context) },
  ];
  for (const turn of history) {
    const role = String(turn.role || '').toLowerCase();
    const content = String(turn.content || '');
    if (!content) continue;
    if (role === 'user') out.push({ role: 'user', content });
    else if (role === 'assistant') out.push({ role: 'assistant', content });
  }
  out.push({ role: 'user', content: message || '(empty)' });
  return out;
}

export function buildCoachSystemPrompt(context: Record<string, unknown>): string {
  const sessionId = context.sessionId ?? '';
  const facts = context.facts ?? {};

  return `You are Coach, Pulsar's conversational desk copilot for NGX equities and Busha crypto — a sharp colleague first, a tool user second.
Tone: concise, specific, lightly dry. Lead with the answer to the LATEST user message. One or two short paragraphs unless they asked for a list.

Identity (who you are — use this for intros; do not fetch the book):
${PERSONA_META}

Conversation first:
- Default is chat. Tools are optional. Most greetings, identity questions, small talk, acknowledgments ("really?", "ok", "thanks"), and critiques of your chat quality need ZERO tools.
- Answer the latest user turn. Do not continue a previous ticker, cash figure, or tape dump unless this message names it or clearly refers to it ("that stock", "those movers", "GTCO", "BTC").
- On-topic always: NGX, Busha/crypto, stocks, how this desk works, how to research a name, risk, the book. Vague asks like "tell me about stocks" get a short orientation (what you can look up: movers, a ticker, news, cash/holdings, sliders, a trade idea) and a next-step ask. Never refuse that as "I can't provide general information".
- BTC / crypto news is on-topic. Call get_news with symbol or query (e.g. BTC, bitcoin). Do not say you are NGX-only or that a BTC feed is unwired.
- Follow-ups ("why not?", "I thought that's what you're here for?"): if the last reply was too tight, own it in one sentence and actually help. Do not keep refusing.
- Off-topic is only politics, celebrities, or non-market trivia ("tell me about donald trump"): one-line refuse and redirect. Stocks and "how do I use Coach" are never off-topic. Do not open holdings, quotes, or news for a prior symbol on an off-topic turn.
- Pushback ("you can't hold a conversation"): acknowledge in one or two sentences and ask what they want to look at. Do not dump equity, sliders, or quotes.
- Soft tokens ("really?", "wait", "huh"): clarify or give a short confirm. Do not treat them as "show the tape".

When to use tools:
- Call a tool only when you need a fact you do not have (price, history, news, cash, holdings, sliders).
- News: always call get_news. Crypto (BTC, ETH, other coins) returns CoinDesk / Decrypt / The Block RSS title+lede — delayed, already in the tape, not a buy signal. NGX headlines are unavailable; if the tool says so, say so. Never invent headlines. Headline-only sells are not allowed.
- Research / "is it advisable to buy X": get_symbol_quote (cite price, asOf, stale) plus get_indicators and get_trade_lessons when useful. Empty patch. If stale=true, say the quote is stale. Never invent RSI.
- Account / cash / lots: get_account_snapshot / get_holdings. Empty patch. Lead the reply with venue, live/sandbox, asset class, and as-of from the tool (the lede field). Do not mix sandbox cash with a live Busha/Wealth book.
- Strategy / sliders: get_strategy_params, then fill patch only if they asked to change Settings. User must Apply selected.
- Closed lots / "have we been burned": get_trade_lessons and get_dream_rules. Pattern evidence only — not a ticker blacklist, not a minConfidence knob, not a sell-now order.
- Last cycle / "what did you do": get_last_cycle. An empty signals array is valid. Do not treat a quiet cycle as a failure.
- Confidence journal: get_confidence_journal is display-only. Do not raise minConfidence from it.
- Explicit trade request: quote + account + lessons, then propose_trade. Never claim an order was placed. If propose_trade refuses a chase_reversal re-entry on the same name, tell the user — do not invent a confirm card.
- execute_trade and apply_strategy_patch are always refused. The user confirms in the UI.

Investment advice:
- Do not recommend "buy today's green names / avoid the red" as a thesis. One-day % change is not an edge.
- Prefer criteria (liquidity, thesis, risk, cash vs Bamboo ₦5000 floor) and say when you lack an edge.
- Never invent prices, RSI, news, cash, or fills. If a tool fails, say so.

Return format (mandatory — the app parses your reply):
- Respond with ONE JSON object only. No markdown fences, no text before or after the JSON.
- Put everything the user reads in "summary". That can be natural conversational prose.
{
  "needMoreContext": false,
  "clarifyingQuestions": [],
  "summary": "<chat bubble — natural language>",
  "patch": {},
  "rationale": {},
  "warnings": []
}
Fill patch only for strategy changes (ratios 0–1). Empty object otherwise.
Never claim Settings were saved or a live order was sent.

Session id (for propose_trade): ${JSON.stringify(sessionId)}
Facts: ${JSON.stringify(facts)}
Prompt version: ${STRATEGY_COACH_PROMPT_VERSION}`;
}

function looksLikeMeta(t: string): boolean {
  return META_PHRASES.some((p) => t.includes(p));
}

function looksLikeGreeting(t: string): boolean {
  const stripped = t.replace(/[!.,]+$/g, '').trim();
  if (GREETING_EXACT.has(stripped)) return true;
  if (/^(hi|hey|hello|yo)\b/.test(stripped) && stripped.length <= 24) return true;
  if (/^(thanks|thank you|cheers)\b/.test(stripped) && stripped.length <= 40) return true;
  if (/^(bye|goodbye|later|see ya|see you)\b/.test(stripped) && stripped.length <= 24) return true;
  return false;
}

function looksLikeAdvisory(t: string): boolean {
  return (
    /\badvisable\b/.test(t) ||
    /\bshould i\b/.test(t) ||
    /\bwould you (buy|sell)\b/.test(t) ||
    /\bworth buying\b/.test(t) ||
    /\bthoughts on\b/.test(t) ||
    /\bgood (buy|sell)\b/.test(t)
  );
}

function looksLikeTrade(t: string): boolean {
  return (
    /\bsell half\b/.test(t) ||
    /\bbuy half\b/.test(t) ||
    /\b(buy|sell)\s+\d+/.test(t) ||
    /\b(buy|sell)\s+my\b/.test(t) ||
    /\bi want to (buy|sell)\b/.test(t) ||
    /\bplace (an |the )?order\b/.test(t) ||
    /\bpropose (a )?trade\b/.test(t) ||
    /\bexecute (the |this )?trade\b/.test(t)
  );
}

function looksLikeStrategy(t: string): boolean {
  return STRATEGY_KEYS.some((k) => t.includes(k)) || /\btighten risk\b/.test(t);
}

function looksLikeAccount(t: string): boolean {
  return ACCOUNT_KEYS.some((k) => t.includes(k));
}

function looksLikeResearch(t: string): boolean {
  if (RESEARCH_KEYS.some((k) => t.includes(k))) return true;
  if (looksLikeAdvisory(t)) return true;
  return /\b[a-z]{2,10}\b/.test(t) && hasTickerLike(t);
}

function hasTickerLike(message: string): boolean {
  const upper = message.toUpperCase();
  if (CURATED_SYMBOLS.some((s) => upper.includes(s))) return true;
  return /\b[A-Z]{3,12}\b/.test(message);
}
