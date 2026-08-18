import { HumanMessage, ToolMessage, type BaseMessage } from '@langchain/core/messages';
import { DynamicStructuredTool } from '@langchain/core/tools';
import {
  LlmPortfolioSignalOutputSchema,
  LlmSignalOutputSchema,
  LlmStrategyCoachOutputSchema,
  type LlmConfig,
} from '@ngx/shared';
import { z } from 'zod';
import { createChatModel } from './model-factory';
import { buildSignalPrompt } from './prompt/v1.0.0';
import { buildPortfolioSignalPrompt } from './prompt/v2.4.0';
import { buildStrategyCoachPrompt } from './prompt/strategy-coach';

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

function coachTools(callTool: ToolCaller): any[] {
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

  return [
    t('get_account_snapshot', 'Cash, equity, venue, live/sandbox, Bamboo floor.', z.object({})),
    t('get_holdings', 'Open lots with avg cost, last, unrealized PnL.', z.object({})),
    t('get_recent_trades', 'Recent fills.', z.object({ limit: z.number().int().optional() })),
    t('get_strategy_params', 'Current Settings sliders.', z.object({})),
    t(
      'list_universe_quotes',
      'Cached quotes for active NGX names: price, percent change, volume, as-of.',
      z.object({ limit: z.number().int().optional() }),
    ),
    t(
      'get_symbol_quote',
      'One symbol live/cached quote.',
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
      'Search on-device memories.',
      z.object({
        query: z.string().min(1),
        symbol: z.string().optional(),
        k: z.number().int().optional(),
      }),
    ),
    t(
      'get_news',
      'Headlines for a symbol or the market. May be unavailable — never fabricate.',
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
  },
): Promise<{ output: unknown; prompt: string; rawResponse: string; modelName: string }> {
  const modelName = `${config.provider}:${config.model}`;
  let rawResponse = '';
  const maxRounds = opts?.maxRounds ?? MAX_TOOL_ROUNDS;
  const maxCalls = opts?.maxCalls ?? MAX_TOOL_CALLS;
  const makeTools = opts?.tools ?? memoryTools;
  const forceHint =
    opts?.forceHint ??
    'Tool budget exhausted. Do not call tools. Return the JSON signals object now.';

  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      if (callTool) {
        const tools = makeTools(callTool);
        const bound = createChatModel(config).bindTools(tools);
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
              const finalModel = createChatModel(config);
              const finalResponse = await finalModel.invoke(messages);
              rawResponse = contentToText(finalResponse.content);
              const parsed = parseJson(rawResponse);
              const validated = validate(parsed);
              return { output: validated, prompt, rawResponse, modelName };
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
              const finalModel = createChatModel(config);
              const finalResponse = await finalModel.invoke(messages);
              rawResponse = contentToText(finalResponse.content);
              const parsed = parseJson(rawResponse);
              const validated = validate(parsed);
              return { output: validated, prompt, rawResponse, modelName };
            }
            continue;
          }
          rawResponse = contentToText(response.content);
          const parsed = parseJson(rawResponse);
          const validated = validate(parsed);
          return { output: validated, prompt, rawResponse, modelName };
        }
      }

      const model = createChatModel(config);
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
  );
}

export async function generateSymbolSignal(
  context: Record<string, unknown>,
  llm: LlmConfig,
) {
  const prompt = buildSignalPrompt(context);
  return invokeWithRetry(llm, prompt, (parsed) => LlmSignalOutputSchema.parse(parsed));
}

export async function generateStrategyCoach(
  context: Record<string, unknown>,
  llm: LlmConfig,
  callTool?: ToolCaller,
) {
  const prompt = buildStrategyCoachPrompt(context);
  return invokeWithRetry(
    llm,
    prompt,
    (parsed) => LlmStrategyCoachOutputSchema.parse(parsed),
    callTool,
    {
      tools: coachTools,
      maxRounds: COACH_TOOL_ROUNDS,
      maxCalls: COACH_TOOL_CALLS,
      forceHint:
        'Tool budget exhausted. Do not call tools. Return the JSON copilot object (summary/patch) now. Do not invent prices.',
    },
  );
}

export async function testLlmConnection(llm: LlmConfig): Promise<string> {
  const model = createChatModel(llm);
  const response = await model.invoke('Reply with exactly: OK');
  return typeof response.content === 'string' ? response.content : JSON.stringify(response.content);
}
