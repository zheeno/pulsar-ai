import { AIMessage, HumanMessage, SystemMessage, ToolMessage, type BaseMessage } from '@langchain/core/messages';
import { DynamicStructuredTool } from '@langchain/core/tools';
import {
  LlmPortfolioSignalOutputSchema,
  LlmSignalOutputSchema,
  LlmStrategyCoachOutputSchema,
  type LlmConfig,
} from '@ngx/shared';
import { z } from 'zod';
import { buildCoachMessages, gateToolCall, parseCoachOutput } from './coach-brain';
import { createChatModel } from './model-factory';
import { buildSignalPrompt } from './prompt/v1.0.0';
import { buildPortfolioSignalPrompt } from './prompt/v2.5.0';

const MAX_TOOL_ROUNDS = 3;
/** Prompt allows 1 search + 2 upserts; hard-cap total invocations across rounds. */
const MAX_TOOL_CALLS = 3;
const COACH_TOOL_ROUNDS = 8;
const COACH_TOOL_CALLS = 8;

export type ToolCaller = (
  name: string,
  args: Record<string, unknown>,
) => Promise<unknown>;

function parseJson(text: string): unknown {
  const jsonMatch = text.match(/\{[\s\S]*\}/);
  if (!jsonMatch) throw new Error('No JSON found in LLM response');
  return JSON.parse(jsonMatch[0]);
}

function contentToText(content: unknown): string {
  if (typeof content === 'string') return content;
  if (Array.isArray(content)) {
    return content
      .map((part) => {
        if (typeof part === 'string') return part;
        if (part && typeof part === 'object' && 'text' in part) {
          return String((part as { text?: unknown }).text ?? '');
        }
        return '';
      })
      .join('')
      .trim();
  }
  if (content && typeof content === 'object' && 'text' in content) {
    return String((content as { text?: unknown }).text ?? '').trim();
  }
  return JSON.stringify(content);
}

function memoryTools(callTool: ToolCaller): any[] {
  return [
    new DynamicStructuredTool({
      name: 'memory_search',
      description:
        'Search on-device agent memory for symbol lessons and notes. Use when retrievedMemories is insufficient.',
      schema: z.object({
        query: z.string().min(1),
        symbol: z.string().optional(),
        k: z.number().int().min(1).max(40).optional(),
      }) as any,
      func: async (input: { query: string; symbol?: string; k?: number }) =>
        JSON.stringify(await callTool('memory_search', input)),
    } as any),
    new DynamicStructuredTool({
      name: 'memory_upsert',
      description:
        'Store a durable lesson (not raw prices). kind is symbol_lesson or freeform.',
      schema: z.object({
        text: z.string().min(1),
        kind: z.enum(['symbol_lesson', 'freeform']),
        symbol: z.string().optional(),
      }) as any,
      func: async (input: { text: string; kind: 'symbol_lesson' | 'freeform'; symbol?: string }) =>
        JSON.stringify(await callTool('memory_upsert', input)),
    } as any),
  ];
}

function coachTools(callTool: ToolCaller, allowed?: string[]): any[] {
  const t = (
    name: string,
    description: string,
    schema: z.ZodTypeAny,
  ) =>
    new DynamicStructuredTool({
      name,
      description,
      schema: schema as any,
      func: async (input: Record<string, unknown>) =>
        JSON.stringify(await callTool(name, input ?? {})),
    } as any);

  const all = [
    t(
      'get_account_snapshot',
      'Cash, equity, venue, live/sandbox, asset class. Lead replies with the lede field (venue + mode + as-of).',
      z.object({}),
    ),
    t('get_holdings', 'Open lots with avg cost, last, unrealized PnL.', z.object({})),
    t('get_recent_trades', 'Recent fills.', z.object({ limit: z.number().int().optional() })),
    t('get_strategy_params', 'Current Settings sliders.', z.object({})),
    t(
      'list_universe_quotes',
      'Cached quotes for the active desk: NGX instruments or Busha pairs. Each row has asOf and stale. Not a buy list.',
      z.object({ limit: z.number().int().optional() }),
    ),
    t(
      'get_symbol_quote',
      'One cached quote with asOf, stale, and indicators when history exists. Never invent a price.',
      z.object({ symbol: z.string().min(1) }),
    ),
    t(
      'get_price_history',
      'Close series for a symbol.',
      z.object({ symbol: z.string().min(1), days: z.number().int().optional() }),
    ),
    t(
      'get_indicators',
      'RSI/SMA from local history. Never invent RSI if this fails.',
      z.object({ symbol: z.string().min(1) }),
    ),
    t(
      'search_memory',
      'Search on-device LESSON / DREAM memories. Not a blacklist.',
      z.object({
        query: z.string().min(1),
        symbol: z.string().optional(),
        k: z.number().int().optional(),
      }),
    ),
    t(
      'get_news',
      'Crypto: CoinDesk/Decrypt/The Block RSS title+lede. Pass symbol=BTC or query=bitcoin. NGX news is unavailable. Never fabricate.',
      z.object({ symbol: z.string().optional(), query: z.string().optional() }),
    ),
    t(
      'run_symbol_screen',
      'Top movers from cached quotes.',
      z.object({ kind: z.string().optional() }),
    ),
    t(
      'explain_blocked_reason',
      'Last BLOCKED_* for a symbol.',
      z.object({ symbol: z.string().min(1) }),
    ),
    t('get_cycle_status', 'Scheduler / live / halt-buys flags.', z.object({})),
    t(
      'get_trade_lessons',
      'Last closed lots as patterns with repeatCount. Not a ticker ban. Do not raise minConfidence.',
      z.object({ limit: z.number().int().optional() }),
    ),
    t(
      'get_dream_rules',
      'Overnight standing cautions (adverse patterns, n>=3). New buys only. Not a ban or sell-now.',
      z.object({}),
    ),
    t(
      'get_last_cycle',
      'Most recent cycle audit (signals, executed, blocked histogram). Empty cycle is valid.',
      z.object({}),
    ),
    t(
      'get_confidence_journal',
      'Closed-lot hit rate by confidence bucket. Display only — do not raise minConfidence.',
      z.object({}),
    ),
    t(
      'propose_strategy_patch',
      'Preview a slider patch. Does not save.',
      z.object({ patch: z.record(z.number()).optional() }).passthrough(),
    ),
    t(
      'propose_trade',
      'Create a trade proposal card. Does not place an order.',
      z.object({
        symbol: z.string().min(1),
        side: z.enum(['BUY', 'SELL']),
        quantity: z.number().optional(),
        notional: z.number().optional(),
        rationale: z.string().optional(),
        sessionId: z.string().optional(),
      }),
    ),
    t(
      'execute_trade',
      'Refused. User must Confirm in the UI.',
      z.object({ proposalId: z.string().optional() }),
    ),
    t(
      'apply_strategy_patch',
      'Refused. User must Apply selected in the UI.',
      z.object({}).passthrough(),
    ),
    ...memoryTools(callTool),
  ];
  if (!allowed) return all;
  return all.filter((tool) => allowed.includes(tool.name));
}

function toBaseMessages(
  turns: { role: 'system' | 'user' | 'assistant'; content: string }[],
): BaseMessage[] {
  return turns.map((t) => {
    if (t.role === 'system') return new SystemMessage(t.content);
    if (t.role === 'assistant') return new AIMessage(t.content);
    return new HumanMessage(t.content);
  });
}

async function invokeCoachMessages(
  config: LlmConfig,
  context: Record<string, unknown>,
  callTool: ToolCaller | undefined,
): Promise<{ output: unknown; prompt: string; rawResponse: string; modelName: string }> {
  const modelName = `${config.provider}:${config.model}`;
  const layout = buildCoachMessages(context);
  const prompt = layout.map((m) => `${m.role}: ${m.content}`).join('\n\n');
  const forceHint =
    'Do not call tools. Return the JSON copilot object now. summary is GitHub-flavored markdown the user reads — no JSON, no fences, no tool dumps. Empty patch unless this turn is a strategy change. Do not invent prices.';

  const gated: ToolCaller | undefined = callTool
    ? async (name, args) => gateToolCall('other', name, args, callTool)
    : undefined;

  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      if (gated) {
        const tools = coachTools(gated);
        const bound = createChatModel(config).bindTools(tools);
        const messages: BaseMessage[] = toBaseMessages(layout);
        let toolCallsUsed = 0;
        for (let round = 0; round <= COACH_TOOL_ROUNDS; round++) {
          const response = await bound.invoke(messages);
          const toolCalls = response.tool_calls as
            | { name: string; args?: Record<string, unknown>; id?: string }[]
            | undefined;
          if (toolCalls && toolCalls.length > 0) {
            const budgetLeft = COACH_TOOL_CALLS - toolCallsUsed;
            messages.push(response as BaseMessage);
            const slice = toolCalls.slice(0, Math.max(0, budgetLeft));
            for (const tc of slice) {
              const result = await gated(tc.name, tc.args ?? {});
              toolCallsUsed += 1;
              messages.push(
                new ToolMessage({
                  content: JSON.stringify(result),
                  tool_call_id: tc.id || tc.name,
                }),
              );
            }
            for (const tc of toolCalls.slice(slice.length)) {
              messages.push(
                new ToolMessage({
                  content: JSON.stringify({
                    ok: false,
                    refused: true,
                    error: 'tool budget exhausted — finish without more tools',
                  }),
                  tool_call_id: tc.id || tc.name,
                }),
              );
            }
            if (toolCallsUsed >= COACH_TOOL_CALLS || round === COACH_TOOL_ROUNDS) {
              messages.push(new HumanMessage(forceHint));
              const finalResponse = await createChatModel(config).invoke(messages);
              const rawResponse = contentToText(finalResponse.content);
              return {
                output: parseCoachOutput(rawResponse),
                prompt,
                rawResponse,
                modelName,
              };
            }
            continue;
          }
          const rawResponse = contentToText(response.content);
          return {
            output: parseCoachOutput(rawResponse),
            prompt,
            rawResponse,
            modelName,
          };
        }
      }

      const response = await createChatModel(config).invoke(toBaseMessages(layout));
      const rawResponse = contentToText(response.content);
      return {
        output: parseCoachOutput(rawResponse),
        prompt,
        rawResponse,
        modelName,
      };
    } catch (err) {
      if (attempt === 1) throw err;
    }
  }
  throw new Error('LLM invoke failed after retries');
}

async function finishSignalJson(
  config: LlmConfig,
  messages: BaseMessage[],
  validate: (parsed: unknown) => unknown,
  forceHint: string,
  reasoning: boolean,
): Promise<{ output: unknown; rawResponse: string }> {
  const toInvoke = reasoning ? [...messages, new HumanMessage(forceHint)] : messages;
  const finalModel = createChatModel(config, { reasoning });
  const finalResponse = await finalModel.invoke(toInvoke);
  const rawResponse = contentToText(finalResponse.content);
  return { output: validate(parseJson(rawResponse)), rawResponse };
}

async function invokeWithRetry(
  config: LlmConfig,
  prompt: string,
  validate: (parsed: unknown) => unknown,
  callTool?: ToolCaller,
  opts?: {
    tools?: (c: ToolCaller) => any[];
    maxRounds?: number;
    maxCalls?: number;
    forceHint?: string;
    reasoning?: boolean;
  },
): Promise<{ output: unknown; prompt: string; rawResponse: string; modelName: string }> {
  const modelName = `${config.provider}:${config.model}`;
  let rawResponse = '';
  const maxRounds = opts?.maxRounds ?? MAX_TOOL_ROUNDS;
  const maxCalls = opts?.maxCalls ?? MAX_TOOL_CALLS;
  const makeTools = opts?.tools ?? memoryTools;
  const useReasoning = Boolean(opts?.reasoning);
  const forceHint =
    opts?.forceHint ??
    'Do not call tools. Return the JSON signals object now.';

  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      if (callTool) {
        const tools = makeTools(callTool);
        const bound = createChatModel(config, { reasoning: false }).bindTools(tools);
        const messages: BaseMessage[] = [new HumanMessage(prompt)];
        let toolCallsUsed = 0;
        for (let round = 0; round <= maxRounds; round++) {
          const response = await bound.invoke(messages);
          const toolCalls = response.tool_calls as
            | { name: string; args?: Record<string, unknown>; id?: string }[]
            | undefined;
          if (toolCalls && toolCalls.length > 0) {
            const budgetLeft = maxCalls - toolCallsUsed;
            if (budgetLeft <= 0 || round === maxRounds) {
              messages.push(response as BaseMessage);
              for (const tc of toolCalls) {
                messages.push(
                  new ToolMessage({
                    content: JSON.stringify({
                      ok: false,
                      error: 'tool budget exhausted — finish without more tools',
                    }),
                    tool_call_id: tc.id || tc.name,
                  }),
                );
              }
              messages.push(new HumanMessage(forceHint));
              const finished = await finishSignalJson(
                config,
                messages,
                validate,
                forceHint,
                useReasoning,
              );
              return {
                output: finished.output,
                prompt,
                rawResponse: finished.rawResponse,
                modelName,
              };
            }
            messages.push(response as BaseMessage);
            for (const tc of toolCalls.slice(0, budgetLeft)) {
              const result = await callTool(tc.name, tc.args ?? {});
              toolCallsUsed += 1;
              messages.push(
                new ToolMessage({
                  content: JSON.stringify(result),
                  tool_call_id: tc.id || tc.name,
                }),
              );
            }
            for (const tc of toolCalls.slice(budgetLeft)) {
              messages.push(
                new ToolMessage({
                  content: JSON.stringify({
                    ok: false,
                    error: 'tool budget exhausted — finish without more tools',
                  }),
                  tool_call_id: tc.id || tc.name,
                }),
              );
            }
            if (toolCallsUsed >= maxCalls || toolCalls.length > budgetLeft) {
              messages.push(new HumanMessage(forceHint));
              const finished = await finishSignalJson(
                config,
                messages,
                validate,
                forceHint,
                useReasoning,
              );
              return {
                output: finished.output,
                prompt,
                rawResponse: finished.rawResponse,
                modelName,
              };
            }
            continue;
          }
          messages.push(response as BaseMessage);
          if (useReasoning) {
            const finished = await finishSignalJson(
              config,
              messages,
              validate,
              forceHint,
              true,
            );
            return {
              output: finished.output,
              prompt,
              rawResponse: finished.rawResponse,
              modelName,
            };
          }
          rawResponse = contentToText(response.content);
          const parsed = parseJson(rawResponse);
          const validated = validate(parsed);
          return { output: validated, prompt, rawResponse, modelName };
        }
      }

      const model = createChatModel(config, { reasoning: useReasoning });
      const response = await model.invoke(prompt);
      rawResponse = contentToText(response.content);
      const parsed = parseJson(rawResponse);
      const validated = validate(parsed);
      return { output: validated, prompt, rawResponse, modelName };
    } catch (err) {
      if (attempt === 1) throw err;
    }
  }

  throw new Error('LLM invoke failed after retries');
}

export async function generatePortfolioSignals(
  context: Record<string, unknown>,
  llm: LlmConfig,
  callTool?: ToolCaller,
) {
  const prompt = buildPortfolioSignalPrompt(context);
  return invokeWithRetry(
    llm,
    prompt,
    (parsed) => LlmPortfolioSignalOutputSchema.parse(parsed),
    callTool,
    { reasoning: true },
  );
}

export async function generateSymbolSignal(
  context: Record<string, unknown>,
  llm: LlmConfig,
) {
  const prompt = buildSignalPrompt(context);
  return invokeWithRetry(llm, prompt, (parsed) => LlmSignalOutputSchema.parse(parsed), undefined, {
    reasoning: true,
  });
}

export async function generateStrategyCoach(
  context: Record<string, unknown>,
  llm: LlmConfig,
  callTool?: ToolCaller,
) {
  return invokeCoachMessages(llm, context, callTool);
}

export async function testLlmConnection(llm: LlmConfig): Promise<string> {
  const model = createChatModel(llm);
  const response = await model.invoke('Reply with exactly: OK');
  return typeof response.content === 'string' ? response.content : JSON.stringify(response.content);
}
