import { ChatAnthropic } from '@langchain/anthropic';
import { ChatOpenAI } from '@langchain/openai';
import type { LlmConfig } from '@ngx/shared';

export type ChatModelOpts = {
  /** When true, omit the gpt-5.6 Chat Completions reasoning_effort=none override. */
  reasoning?: boolean;
};

function temperatureOpts(config: LlmConfig): { temperature?: number } {
  if (config.temperature == null) return {};
  return { temperature: config.temperature };
}

/**
 * GPT-5.6 (luna/sol/terra) defaults to a non-none reasoning effort. Chat Completions
 * rejects function tools unless reasoning_effort is "none" (or the request uses /v1/responses).
 * Force "none" unless this invoke is a no-tools reasoning pass (BUY/SELL/HOLD JSON).
 */
function needsChatCompletionsReasoningNone(model: string): boolean {
  return /gpt-5\.6/i.test(model);
}

function openaiReasoningOpts(
  config: LlmConfig,
  opts?: ChatModelOpts,
): Record<string, unknown> {
  if (opts?.reasoning) return {};
  if (!needsChatCompletionsReasoningNone(config.model)) return {};
  return { modelKwargs: { reasoning_effort: 'none' } };
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function createChatModel(config: LlmConfig, opts?: ChatModelOpts): any {
  const temp = temperatureOpts(config);
  const reasoning = openaiReasoningOpts(config, opts);

  switch (config.provider) {
    case 'anthropic':
      return new ChatAnthropic({
        anthropicApiKey: config.apiKey,
        modelName: config.model,
        ...temp,
      });
    case 'openrouter':
      return new ChatOpenAI({
        openAIApiKey: config.apiKey,
        modelName: config.model,
        ...temp,
        ...reasoning,
        configuration: {
          baseURL: config.baseUrl || 'https://openrouter.ai/api/v1',
        },
      });
    case 'openai':
    default:
      return new ChatOpenAI({
        openAIApiKey: config.apiKey,
        modelName: config.model,
        ...temp,
        ...reasoning,
        ...(config.baseUrl ? { configuration: { baseURL: config.baseUrl } } : {}),
      });
  }
}
