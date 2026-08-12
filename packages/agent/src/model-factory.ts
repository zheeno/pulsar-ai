import { ChatAnthropic } from '@langchain/anthropic';
import { ChatOpenAI } from '@langchain/openai';
import type { LlmConfig } from '@ngx/shared';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function createChatModel(config: LlmConfig): any {
  const temperature = 0.1;

  switch (config.provider) {
    case 'anthropic':
      return new ChatAnthropic({
        anthropicApiKey: config.apiKey,
        modelName: config.model,
        temperature,
      });
    case 'openrouter':
      return new ChatOpenAI({
        openAIApiKey: config.apiKey,
        modelName: config.model,
        temperature,
        configuration: {
          baseURL: config.baseUrl || 'https://openrouter.ai/api/v1',
        },
      });
    case 'openai':
    default:
      return new ChatOpenAI({
        openAIApiKey: config.apiKey,
        modelName: config.model,
        temperature,
        ...(config.baseUrl ? { configuration: { baseURL: config.baseUrl } } : {}),
      });
  }
}
