import { Injectable, Logger } from '@nestjs/common';
import { ChatOpenAI } from '@langchain/openai';
import { LlmSignalOutputSchema, LlmPortfolioSignalOutputSchema, PROMPT_VERSION } from '@ngx/shared';
import { buildSignalPrompt } from './prompt/v1.0.0';
import { buildPortfolioSignalPrompt } from './prompt/v2.0.0';
import { logStart } from '../common/log.util';

@Injectable()
export class LlmService {
  private readonly logger = new Logger(LlmService.name);
  private readonly useMock: boolean;
  private model: ChatOpenAI | null = null;

  constructor() {
    const apiKey = process.env.OPENAI_API_KEY || '';
    this.useMock = !apiKey || apiKey === 'your-openai-api-key';
    if (!this.useMock) {
      this.model = new ChatOpenAI({
        modelName: process.env.OPENAI_MODEL || 'gpt-4o-mini',
        temperature: 0.1,
        openAIApiKey: apiKey,
      });
    } else {
      this.logger.warn('OpenAI API key not set — using mock LLM responses');
    }
  }

  async generateSignal(context: Record<string, unknown>): Promise<{
    output: { action: 'BUY' | 'SELL' | 'HOLD'; confidence: number; rationale: string };
    prompt: string;
    rawResponse: string;
    modelName: string;
  }> {
    const symbol = context.symbol as string | undefined;
    const log = logStart(this.logger, 'generateSignal', { symbol, mock: this.useMock });
    const prompt = buildSignalPrompt(context);
    const modelName = process.env.OPENAI_MODEL || 'gpt-4o-mini';

    if (this.useMock) {
      const output = this.mockResponse(context);
      log.done({ action: output.action, confidence: output.confidence, model: 'mock-llm' });
      return { output, prompt, rawResponse: JSON.stringify(output), modelName: 'mock-llm' };
    }

    let rawResponse = '';
    for (let attempt = 0; attempt < 2; attempt++) {
      try {
        const response = await this.model!.invoke(prompt);
        rawResponse = typeof response.content === 'string' ? response.content : JSON.stringify(response.content);
        const parsed = this.parseJson(rawResponse);
        const validated = LlmSignalOutputSchema.parse(parsed);
        log.done({ action: validated.action, confidence: validated.confidence, model: modelName });
        return { output: validated, prompt, rawResponse, modelName };
      } catch (err) {
        log.warn(`parse attempt ${attempt + 1} failed`, { error: err instanceof Error ? err.message : String(err) });
      }
    }

    const fallback = { action: 'HOLD' as const, confidence: 0, rationale: 'LLM output invalid after retries' };
    log.warn('using fallback HOLD');
    log.done({ action: 'HOLD', confidence: 0 });
    return { output: fallback, prompt, rawResponse, modelName };
  }

  async generatePortfolioSignals(context: Record<string, unknown>): Promise<{
    output: { signals: { symbol: string; action: 'BUY' | 'SELL' | 'HOLD'; confidence: number; rationale: string }[] };
    prompt: string;
    rawResponse: string;
    modelName: string;
  }> {
    const log = logStart(this.logger, 'generatePortfolioSignals', { mock: this.useMock });
    const prompt = buildPortfolioSignalPrompt(context);
    const modelName = process.env.OPENAI_MODEL || 'gpt-4o-mini';

    if (this.useMock) {
      const output = this.mockPortfolioResponse(context);
      log.done({ count: output.signals.length, model: 'mock-llm' });
      return { output, prompt, rawResponse: JSON.stringify(output), modelName: 'mock-llm' };
    }

    let rawResponse = '';
    for (let attempt = 0; attempt < 2; attempt++) {
      try {
        const response = await this.model!.invoke(prompt);
        rawResponse = typeof response.content === 'string' ? response.content : JSON.stringify(response.content);
        const parsed = this.parseJson(rawResponse);
        const validated = LlmPortfolioSignalOutputSchema.parse(parsed);
        log.done({ count: validated.signals.length, model: modelName });
        return { output: validated, prompt, rawResponse, modelName };
      } catch (err) {
        log.warn(`portfolio parse attempt ${attempt + 1} failed`, {
          error: err instanceof Error ? err.message : String(err),
        });
      }
    }

    const fallback = { signals: [] as { symbol: string; action: 'HOLD'; confidence: number; rationale: string }[] };
    log.warn('using empty portfolio fallback');
    log.done({ count: 0 });
    return { output: fallback, prompt, rawResponse, modelName };
  }

  private mockPortfolioResponse(context: Record<string, unknown>): {
    signals: { symbol: string; action: 'BUY' | 'SELL' | 'HOLD'; confidence: number; rationale: string }[];
  } {
    const universe = (context.universe as Array<{
      symbol: string;
      price?: number;
      change_percent?: number;
      volume?: number;
    }>) || [];
    const positions = (context.positions as Array<{ symbol: string; quantity: number }>) || [];
    const maxPicks = Number(context.maxPicks || 5);
    const signals: { symbol: string; action: 'BUY' | 'SELL' | 'HOLD'; confidence: number; rationale: string }[] = [];
    const held = new Set(positions.map((p) => p.symbol));

    const ranked = [...universe]
      .filter((s) => s.price && s.price > 0)
      .sort((a, b) => (b.change_percent || 0) - (a.change_percent || 0));

    for (const stock of ranked.filter((s) => (s.change_percent || 0) > 0.5).slice(0, 2)) {
      signals.push({
        symbol: stock.symbol,
        action: 'BUY',
        confidence: 0.7,
        rationale: `${stock.symbol} up ${(stock.change_percent || 0).toFixed(2)}% with price ${stock.price}`,
      });
    }

    for (const stock of ranked.filter((s) => (s.change_percent || 0) < -1 && held.has(s.symbol)).slice(0, 1)) {
      signals.push({
        symbol: stock.symbol,
        action: 'SELL',
        confidence: 0.66,
        rationale: `${stock.symbol} down ${(stock.change_percent || 0).toFixed(2)}% while held in portfolio`,
      });
    }

    if (signals.length === 0 && ranked[0]) {
      signals.push({
        symbol: ranked[0].symbol,
        action: 'HOLD',
        confidence: 0.45,
        rationale: `No strong setups; ${ranked[0].symbol} is the top mover at ${(ranked[0].change_percent || 0).toFixed(2)}%`,
      });
    }

    return { signals: signals.slice(0, maxPicks) };
  }

  private parseJson(text: string): unknown {
    const jsonMatch = text.match(/\{[\s\S]*\}/);
    if (!jsonMatch) throw new Error('No JSON found');
    return JSON.parse(jsonMatch[0]);
  }

  private mockResponse(context: Record<string, unknown>): { action: 'BUY' | 'SELL' | 'HOLD'; confidence: number; rationale: string } {
    const technical = context.technical as Record<string, number | null> | undefined;
    const rsi = technical?.rsi14 ?? 50;
    const momentum = technical?.momentum ?? 0;
    const sma50 = technical?.sma50 ?? 0;
    const currentPrice = technical?.currentPrice ?? 0;

    if (rsi < 35 && momentum > 0) {
      return { action: 'BUY', confidence: 0.72, rationale: `RSI at ${rsi.toFixed(1)} indicates oversold; momentum ${momentum.toFixed(2)}% positive` };
    }
    if (rsi > 70) {
      return { action: 'SELL', confidence: 0.68, rationale: `RSI at ${rsi.toFixed(1)}, above overbought threshold of 70` };
    }
    if (currentPrice > sma50 && momentum > 1) {
      return { action: 'BUY', confidence: 0.66, rationale: `Price ${currentPrice} above SMA50 ${sma50?.toFixed(2)}; momentum ${momentum.toFixed(2)}%` };
    }
    return { action: 'HOLD', confidence: 0.45, rationale: `Mixed signals: RSI ${rsi.toFixed(1)}, momentum ${momentum.toFixed(2)}%` };
  }
}
