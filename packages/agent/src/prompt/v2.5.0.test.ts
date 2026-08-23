import assert from 'node:assert/strict';
import { test } from 'node:test';
import { buildPortfolioSignalPrompt } from './v2.5.0';

test('portfolio prompt allows an empty signals array and has no buy quota', () => {
  const prompt = buildPortfolioSignalPrompt({
    universeSize: 10,
    universeTsv: 'SYM\tpx\nGTCO\t50',
    executionConstraints: { maxBuySignals: 3, minConfidenceToTrade: 0.65 },
    diversification: { minBuySignals: 8, maxBuysPerSector: 3 },
  });
  assert.match(prompt, /empty signals array is valid/i);
  assert.doesNotMatch(prompt, /Do not return an empty signals array/);
  assert.doesNotMatch(prompt, /Target at least/);
  assert.match(prompt, /One-day % is not an edge/);
});

test('prompt treats closed lots as patterns, not a blacklist', () => {
  const prompt = buildPortfolioSignalPrompt({
    universeSize: 4,
    tradeLessons: [{ symbol: 'HOME', pattern: 'chase_reversal', repeatCount: 3 }],
    dreamRules: [{ pattern: 'chase_reversal', n: 5 }],
  });
  assert.match(prompt, /not a ticker blacklist/);
  assert.match(prompt, /repeatCount >= 3/);
  assert.match(prompt, /Do not raise minConfidence/);
  assert.match(prompt, /dreamRules are overnight consolidations/);
  assert.match(prompt, /Do not treat a dream rule as a sell-now order/);
});

test('crypto prompt names Busha, not NGX', () => {
  const prompt = buildPortfolioSignalPrompt({
    universeSize: 4,
    executionConstraints: { assetClass: 'crypto', maxBuySignals: 2 },
  });
  assert.match(prompt, /Busha crypto/);
  assert.doesNotMatch(prompt, /You are a NGX/);
});

test('crypto prompt treats RSS headlines as context, not a buy signal', () => {
  const prompt = buildPortfolioSignalPrompt({
    universeSize: 4,
    executionConstraints: { assetClass: 'crypto', maxBuySignals: 2 },
    news: {
      status: 'ok',
      headlines: [{ source: 'CoinDesk', title: 'Bitcoin ETF inflows', summary: 'Weekly inflows rose.' }],
    },
  });
  assert.match(prompt, /title plus a short lede/);
  assert.match(prompt, /Do not BUY because a headline is bullish/);
  assert.match(prompt, /Headline-only sells are not allowed/);
});
