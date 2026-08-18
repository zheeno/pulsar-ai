# @ngx/agent

LangChain stdio worker for Pulsar AI signal generation.

## Protocol

Line-delimited JSON on stdin/stdout:

```json
{"id":"1","op":"ping"}
{"id":"1","ok":true,"data":{"pong":true,"version":"1.0.0"}}
```

Ops: `ping`, `test_llm`, `portfolio_signals`, `symbol_signal`.

## Build / run

```bash
npm run build -w @ngx/agent
node packages/agent/dist/worker.js
```

Smoke test: `npm run agent:ping`
