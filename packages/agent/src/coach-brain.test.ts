import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  allowedTools,
  attachesBook,
  buildCoachMessages,
  buildCoachSystemPrompt,
  cannedSocialReply,
  classifyCoachIntent,
  gateToolCall,
  isIrreversibleCoachTool,
  parseCoachOutput,
  PERSONA_GREETING,
  PERSONA_META,
  toolAllowed,
} from './coach-brain';

function systemPrompt(message: string, history: { role: string; content: string }[] = []) {
  return buildCoachMessages({
    conversation: { message, history },
    facts: { BAMBOO_MIN_ORDER_NOTIONAL: 5000 },
  })[0].content;
}

test('D1–D2 prompt treats greetings and identity as conversation, not tape', () => {
  const hello = systemPrompt('hello');
  assert.match(hello, /Conversation first/i);
  assert.match(hello, /ZERO tools/i);
  assert.match(hello, /really\?/);
  assert.doesNotMatch(hello, /Allowed tools: \(none/);
  assert.match(hello, /copilot|NGX/i);

  const who = systemPrompt('tell me about you');
  assert.match(who, /Identity/);
  assert.ok(!attachesBook());
  assert.ok(allowedTools('meta').includes('get_symbol_quote'));
  assert.ok(allowedTools('greeting').includes('list_universe_quotes'));
  const canned = cannedSocialReply('greeting', 'hello');
  assert.doesNotMatch(canned, /₦|cash|holdings|gtco/i);
  assert.match(PERSONA_META, /NGX/);
});

test('D3–D5 policy: soft ack, off-topic, critique never dump the book', () => {
  const prompt = systemPrompt('really?', [
    { role: 'user', content: 'hello' },
    { role: 'assistant', content: 'Hey. Tape or a ticker?' },
  ]);
  assert.match(prompt, /Soft tokens/);
  assert.match(prompt, /Do not treat them as "show the tape"/);
  assert.match(prompt, /On-topic always/);
  assert.match(prompt, /tell me about stocks/);
  assert.match(prompt, /Never refuse that/);
  assert.match(prompt, /Off-topic/);
  assert.match(prompt, /donald trump/i);
  assert.match(prompt, /Pushback/);
  assert.match(prompt, /Do not dump equity/);
  assert.doesNotMatch(prompt, /Book context/);
  assert.doesNotMatch(prompt, /5485/);
});

test('D6–D7 research and investment caution stay in the prompt', () => {
  const prompt = systemPrompt('is it advisable to buy GTCO?');
  assert.match(prompt, /get_symbol_quote/);
  assert.match(prompt, /Investment advice/);
  assert.match(prompt, /green names/);
  assert.match(prompt, /not an edge/);
});

test('D8–D9 strategy and trade: confirm in UI, irreversible refused', async () => {
  const prompt = systemPrompt('tighten risk');
  assert.match(prompt, /Apply selected/);
  assert.match(prompt, /propose_trade/);
  assert.match(prompt, /always refused/);
  assert.ok(toolAllowed('strategy', 'propose_strategy_patch'));
  assert.ok(!toolAllowed('strategy', 'apply_strategy_patch'));
  assert.ok(toolAllowed('trade', 'propose_trade'));
  assert.ok(!toolAllowed('trade', 'execute_trade'));
  assert.ok(isIrreversibleCoachTool('execute_trade'));
  assert.ok(isIrreversibleCoachTool('apply_strategy_patch'));

  let called = 0;
  const refused = (await gateToolCall('trade', 'execute_trade', { proposalId: 'x' }, async () => {
    called += 1;
    return { ok: true };
  })) as { ok?: boolean; refused?: boolean };
  assert.equal(called, 0);
  assert.equal(refused.ok, false);
  assert.equal(refused.refused, true);

  const quote = (await gateToolCall('meta', 'get_symbol_quote', { symbol: 'GTCO' }, async () => {
    called += 1;
    return { ok: true, price: 46.2 };
  })) as { ok?: boolean; price?: number };
  assert.equal(called, 1);
  assert.equal(quote.price, 46.2);
});

test('B4 messages stay role-based after a GTCO thread', () => {
  const history = [
    { role: 'user', content: 'is it advisable to buy GTCO?' },
    { role: 'assistant', content: 'GTCO last ₦46.20; cash ₦5485.' },
  ];
  const msgs = buildCoachMessages({
    conversation: { message: 'tell me about yourself', history },
  });
  assert.equal(msgs[0].role, 'system');
  assert.equal(msgs[msgs.length - 1].role, 'user');
  assert.equal(msgs[msgs.length - 1].content, 'tell me about yourself');
  assert.ok(msgs.some((m) => m.role === 'assistant' && m.content.includes('GTCO')));
  assert.doesNotMatch(msgs[0].content, /UNTRUSTED DATA/);
  assert.doesNotMatch(msgs[0].content, /Allowed tools: \(none/);
  assert.match(msgs[0].content, /latest user turn/i);
});

test('B5 messages API is role-based, not a single JSON blob', () => {
  const msgs = buildCoachMessages({
    conversation: {
      message: 'tighten risk',
      history: [
        { role: 'user', content: 'hi' },
        { role: 'assistant', content: 'Hey.' },
      ],
    },
  });
  assert.deepEqual(
    msgs.map((m) => m.role),
    ['system', 'user', 'assistant', 'user'],
  );
  assert.equal(msgs[1].content, 'hi');
  assert.equal(msgs[3].content, 'tighten risk');
});

test('classifier remains a hint, not a tool gate', () => {
  assert.equal(classifyCoachIntent('tell me about yourself'), 'meta');
  assert.equal(classifyCoachIntent('hi'), 'greeting');
  assert.equal(classifyCoachIntent('is it advisable to buy GTCO?'), 'research');
  assert.equal(classifyCoachIntent('tighten risk'), 'strategy');
  assert.equal(classifyCoachIntent('sell half my MTNN'), 'trade');
  assert.ok(allowedTools(classifyCoachIntent('tell me about yourself')).length > 0);
  assert.match(buildCoachSystemPrompt({ conversation: { message: 'hi' } }), /Prompt version:/);
});

test('parseCoachOutput accepts JSON and plain conversational replies', () => {
  const json = parseCoachOutput(
    '{"summary":"Hey there.","patch":{},"needMoreContext":false,"clarifyingQuestions":[],"rationale":{},"warnings":[]}',
  );
  assert.equal(json.summary, 'Hey there.');
  assert.deepEqual(json.patch, {});

  const plain = parseCoachOutput('Hey! What do you want to look at on NGX today?');
  assert.equal(plain.summary, 'Hey! What do you want to look at on NGX today?');
  assert.deepEqual(plain.patch, {});

  const fenced = parseCoachOutput('```json\n{"summary":"Hi from fence","patch":{}}\n```');
  assert.equal(fenced.summary, 'Hi from fence');

  const empty = parseCoachOutput('   ');
  assert.equal(empty.summary, PERSONA_GREETING);
});
