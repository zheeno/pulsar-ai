import {
  LlmPortfolioSignalOutputSchema,
  LlmSignalOutputSchema,
  type LlmConfig,
} from '@ngx/shared';
import { createChatModel } from './model-factory';
import { buildSignalPrompt } from './prompt/v1.0.0';
import { buildPortfolioSignalPrompt } from './prompt/v2.0.0';

function parseJson(text: string): unknown {
  const jsonMatch = text.match(/\{[\s\S]*\}/);
  if (!jsonMatch) throw new Error('No JSON found in LLM response');
  return JSON.parse(jsonMatch[0]);
}

async function invokeWithRetry(
  config: LlmConfig,
  prompt: string,
  validate: (parsed: unknown) => unknown,
): Promise<{ output: unknown; prompt: string; rawResponse: string; modelName: string }> {
  const model = createChatModel(config);
  const modelName = `${config.provider}:${config.model}`;
  let rawResponse = '';

  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      const response = await model.invoke(prompt);
      rawResponse =
        typeof response.content === 'string'
          ? response.content
          : JSON.stringify(response.content);
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
) {
  const prompt = buildPortfolioSignalPrompt(context);
  return invokeWithRetry(llm, prompt, (parsed) => LlmPortfolioSignalOutputSchema.parse(parsed));
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
