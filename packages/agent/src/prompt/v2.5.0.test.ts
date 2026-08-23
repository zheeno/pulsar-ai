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
