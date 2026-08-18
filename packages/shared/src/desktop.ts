import { z } from 'zod';

export const LlmProviderSchema = z.enum(['openai', 'anthropic', 'openrouter']);
export type LlmProvider = z.infer<typeof LlmProviderSchema>;

export const LlmConfigSchema = z.object({
  provider: LlmProviderSchema,
  model: z.string().min(1),
  apiKey: z.string().min(1),
  baseUrl: z.string().url().nullish(),
});
export type LlmConfig = z.infer<typeof LlmConfigSchema>;

export const PulseSettingsSchema = z.object({
  supabaseUrl: z.string().url(),
  supabaseAnonKey: z.string().min(1),
  email: z.string().email(),
  password: z.string().min(1),
  apiKey: z.string().optional(),
  baseUrl: z.string().url().default('https://ngxpulse.ng/api'),
});
export type PulseSettings = z.infer<typeof PulseSettingsSchema>;

export const AppSettingsSchema = z.object({
  llm: LlmConfigSchema.optional(),
  pulseConfigured: z.boolean().default(false),
  llmConfigured: z.boolean().default(false),
  onboardingComplete: z.boolean().default(false),
  defaultStartingCapital: z.number().positive().default(10_000_000),
  simulatedSlippageBps: z.number().default(10),
  simulatedFeePct: z.number().default(0.0015),
  autoCycleEnabled: z.boolean().default(false),
  autoCycleIntervalMinutes: z.number().int().min(5).max(120).default(30),
  liveTradingEnabled: z.boolean().default(false),
  maxLiveNotional: z.number().min(1000).max(50_000_000).default(500_000),
  maxLiveActions: z.number().int().min(1).max(40).default(10),
  retainRawLlmLogs: z.boolean().default(false),
});
export type AppSettings = z.infer<typeof AppSettingsSchema>;

export const AgentToolCallSchema = z.object({
  type: z.literal('tool'),
  name: z.string().min(1),
  arguments: z.record(z.unknown()),
});
export type AgentToolCall = z.infer<typeof AgentToolCallSchema>;

export const AgentToolResultSchema = z.object({
  type: z.literal('tool_result'),
  name: z.string(),
  result: z.unknown(),
});
export type AgentToolResult = z.infer<typeof AgentToolResultSchema>;

export const AgentRequestSchema = z.discriminatedUnion('op', [
  z.object({ id: z.string(), op: z.literal('ping') }),
  z.object({
    id: z.string(),
    op: z.literal('portfolio_signals'),
    context: z.record(z.unknown()),
    llm: LlmConfigSchema,
  }),
  z.object({
    id: z.string(),
    op: z.literal('symbol_signal'),
    context: z.record(z.unknown()),
    llm: LlmConfigSchema,
  }),
  z.object({
    id: z.string(),
    op: z.literal('test_llm'),
    llm: LlmConfigSchema,
  }),
  z.object({
    id: z.string(),
    op: z.literal('strategy_coach'),
    context: z.record(z.unknown()),
    llm: LlmConfigSchema,
  }),
]);
export type AgentRequest = z.infer<typeof AgentRequestSchema>;

export const AgentResponseSchema = z.object({
  id: z.string(),
  ok: z.boolean(),
  error: z.string().optional(),
  data: z.unknown().optional(),
});
export type AgentResponse = z.infer<typeof AgentResponseSchema>;

export const AgentSignalResultSchema = z.object({
  output: z.union([
    z.object({
      action: z.enum(['BUY', 'SELL', 'HOLD']),
      confidence: z.number(),
      rationale: z.string(),
    }),
    z.object({
      signals: z.array(
        z.object({
          symbol: z.string(),
          action: z.enum(['BUY', 'SELL', 'HOLD']),
          confidence: z.number(),
          rationale: z.string(),
        }),
      ),
    }),
  ]),
  prompt: z.string(),
  rawResponse: z.string(),
  modelName: z.string(),
});
export type AgentSignalResult = z.infer<typeof AgentSignalResultSchema>;

export const SECRET_KEYS = {
  PULSE_PASSWORD: 'pulse_password',
  PULSE_API_KEY: 'pulse_api_key',
  LLM_API_KEY: 'llm_api_key',
  WEALTH_PASSWORD: 'wealth_password',
  WEALTH_TOKEN: 'wealth_token',
  WEALTH_TOKEN_EXPIRES: 'wealth_token_expires',
} as const;
