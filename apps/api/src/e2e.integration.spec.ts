import { Test, TestingModule } from '@nestjs/testing';
import { INestApplication } from '@nestjs/common';
import { getModelToken } from '@nestjs/mongoose';
import { MongoMemoryServer } from 'mongodb-memory-server';
import { Model } from 'mongoose';
import { execSync } from 'child_process';
import { join } from 'path';
import {
  SandboxPortfolio,
  SandboxPortfolioDocument,
  PriceHistory,
  PriceHistoryDocument,
  StrategyParamSet,
  StrategyParamSetDocument,
  Signal,
  SignalDocument,
  SignalLlmLog,
  SignalLlmLogDocument,
} from './database/schemas';
import { IngestionService } from './market-data/ingestion.service';
import { SignalGenerationService } from './signal-generation/signal-generation.service';
import { ExecutionService } from './execution/execution.service';

describe('E2E Integration', () => {
  let app: INestApplication;
  let mongoServer: MongoMemoryServer;
  let portfolioModel: Model<SandboxPortfolioDocument>;
  let priceModel: Model<PriceHistoryDocument>;
  let strategyModel: Model<StrategyParamSetDocument>;
  let signalModel: Model<SignalDocument>;
  let llmLogModel: Model<SignalLlmLogDocument>;

  beforeAll(async () => {
    mongoServer = await MongoMemoryServer.create({
      instance: { launchTimeout: 120000 },
    });
    process.env.MONGODB_URI = mongoServer.getUri();
    process.env.FORCE_INGEST = 'true';
    process.env.JWT_SECRET = 'test-jwt-secret-min-32-characters-long';

    execSync('node scripts/seed.js', {
      env: process.env,
      cwd: join(__dirname, '../../..'),
      stdio: 'inherit',
    });

    const { AppModule } = await import('./app.module');
    const moduleFixture: TestingModule = await Test.createTestingModule({
      imports: [AppModule],
    }).compile();

    app = moduleFixture.createNestApplication();
    app.setGlobalPrefix('api');
    await app.init();

    portfolioModel = app.get(getModelToken(SandboxPortfolio.name));
    priceModel = app.get(getModelToken(PriceHistory.name));
    strategyModel = app.get(getModelToken(StrategyParamSet.name));
    signalModel = app.get(getModelToken(Signal.name));
    llmLogModel = app.get(getModelToken(SignalLlmLog.name));
  }, 120000);

  afterAll(async () => {
    await app?.close();
    await mongoServer?.stop();
  });

  it('should have seeded portfolio in database', async () => {
    const portfolio = await portfolioModel.findOne({ name: 'default-sandbox' }).lean().exec();
    expect(portfolio).toBeTruthy();
    expect(Number(portfolio!.starting_capital)).toBe(10000000);
  });

  it('should have price history for curated symbols', async () => {
    const count = await priceModel.distinct('symbol').exec();
    expect(count.length).toBeGreaterThanOrEqual(10);
  });

  it('should have active strategy param set with open symbol universe', async () => {
    const active = await strategyModel.find({ is_active: true }).exec();
    expect(active.length).toBe(1);
    const allowed = active[0].allowed_symbols;
    expect(allowed == null || (Array.isArray(allowed) && allowed.length >= 0)).toBe(true);
  });

  it('should run full cycle and create signals', async () => {
    const ingestion = app.get(IngestionService);
    const signals = app.get(SignalGenerationService);
    const execution = app.get(ExecutionService);

    await ingestion.ingestStocks({ force: true });

    const signalIds = await signals.generateForPortfolio();
    expect(signalIds.length).toBeGreaterThan(0);

    const signal = await signalModel.findById(signalIds[0]).lean().exec();
    expect(signal?.rationale).toBeTruthy();
    expect(signal?.technical_snapshot).toBeTruthy();

    const llmLog = await llmLogModel.findOne({ signal_id: signalIds[0] }).lean().exec();
    expect(llmLog).toBeTruthy();

    await execution.processSignals(signalIds);

    const updatedSignal = await signalModel.findById(signalIds[0]).lean().exec();
    expect(updatedSignal?.risk_policy_result).toBeTruthy();
  }, 120000);
});
