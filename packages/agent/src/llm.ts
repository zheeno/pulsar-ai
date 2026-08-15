import { HumanMessage, ToolMessage, type BaseMessage } from '@langchain/core/messages';
import { DynamicStructuredTool } from '@langchain/core/tools';
import {
  LlmPortfolioSignalOutputSchema,
  LlmSignalOutputSchema,
  type LlmConfig,
} from '@ngx/shared';
import { z } from 'zod';
import { createChatModel } from './model-factory';
import { buildSignalPrompt } from './prompt/v1.0.0';
import { buildPortfolioSignalPrompt } from './prompt/v2.4.0';

const MAX_TOOL_ROUNDS = 6;

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

async function invokeWithRetry(
  config: LlmConfig,
  prompt: string,
  validate: (parsed: unknown) => unknown,
  callTool?: ToolCaller,
): Promise<{ output: unknown; prompt: string; rawResponse: string; modelName: string }> {
  const modelName = `${config.provider}:${config.model}`;
  let rawResponse = '';

  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      if (callTool) {
        const tools = memoryTools(callTool);
        const bound = createChatModel(config).bindTools(tools);
        const messages: BaseMessage[] = [new HumanMessage(prompt)];
        for (let round = 0; round <= MAX_TOOL_ROUNDS; round++) {
          const response = await bound.invoke(messages);
          const toolCalls = response.tool_calls as
            | { name: string; args?: Record<string, unknown>; id?: string }[]
            | undefined;
          if (toolCalls && toolCalls.length > 0) {
            if (round === MAX_TOOL_ROUNDS) {
              throw new Error('Agent exceeded memory tool round limit');
            }
            messages.push(response as BaseMessage);
            for (const tc of toolCalls) {
              const result = await callTool(tc.name, tc.args ?? {});
              messages.push(
                new ToolMessage({
                  content: JSON.stringify(result),
                  tool_call_id: tc.id || tc.name,
                }),
              );
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

export async function testLlmConnection(llm: LlmConfig): Promise<string> {
  const model = createChatModel(llm);
  const response = await model.invoke('Reply with exactly: OK');
  return typeof response.content === 'string' ? response.content : JSON.stringify(response.content);
}
