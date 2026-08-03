import { Test, TestingModule } from '@nestjs/testing';
import { INestApplication } from '@nestjs/common';
import { AppModule } from './app.module';
import { DatabaseService } from './database/database.service';
import { IngestionService } from './market-data/ingestion.service';
import { SignalGenerationService } from './signal-generation/signal-generation.service';
import { ExecutionService } from './execution/execution.service';

describe('E2E Integration', () => {
  let app: INestApplication;
  let db: DatabaseService;

  beforeAll(async () => {
    process.env.FORCE_INGEST = 'true';
    const moduleFixture: TestingModule = await Test.createTestingModule({
      imports: [AppModule],
    }).compile();

    app = moduleFixture.createNestApplication();
    app.setGlobalPrefix('api');
    await app.init();
    db = app.get(DatabaseService);
  }, 30000);

  afterAll(async () => {
    await app.close();
  });

  it('should have seeded portfolio in database', async () => {
    const result = await db.query(`SELECT * FROM sandbox_portfolios WHERE name = 'default-sandbox'`);
    expect(result.rows.length).toBeGreaterThan(0);
    expect(Number(result.rows[0].starting_capital)).toBe(10000000);
  });

  it('should have price history for curated symbols', async () => {
    const result = await db.query(`SELECT COUNT(DISTINCT symbol) as count FROM price_history`);
    expect(Number(result.rows[0].count)).toBeGreaterThanOrEqual(10);
  });

  it('should have active strategy param set', async () => {
    const result = await db.query(`SELECT * FROM strategy_param_sets WHERE is_active = true`);
    expect(result.rows.length).toBe(1);
    expect(result.rows[0].allowed_symbols.length).toBeGreaterThan(0);
  });

  it('should run full cycle and create signals', async () => {
    const ingestion = app.get(IngestionService);
    const signals = app.get(SignalGenerationService);
    const execution = app.get(ExecutionService);

    const ingested = await ingestion.ingestStocks();
    expect(ingested).toBeGreaterThan(0);

    const signalIds = await signals.generateForPortfolio();
    expect(signalIds.length).toBeGreaterThan(0);

    const signalCheck = await db.query('SELECT * FROM signals WHERE id = $1', [signalIds[0]]);
    expect(signalCheck.rows[0].rationale).toBeTruthy();
    expect(signalCheck.rows[0].technical_snapshot).toBeTruthy();

    const llmLog = await db.query('SELECT * FROM signal_llm_logs WHERE signal_id = $1', [signalIds[0]]);
    expect(llmLog.rows.length).toBe(1);

    await execution.processSignals(signalIds);

    const updatedSignal = await db.query('SELECT risk_policy_result FROM signals WHERE id = $1', [signalIds[0]]);
    expect(updatedSignal.rows[0].risk_policy_result).toBeTruthy();
  }, 120000);
});
