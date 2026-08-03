import { Module } from '@nestjs/common';
import { ConfigModule } from '@nestjs/config';
import { ScheduleModule } from '@nestjs/schedule';
import { BullModule } from '@nestjs/bullmq';
import { DatabaseModule } from './database/database.module';
import { RedisModule } from './redis/redis.module';
import { AuthModule } from './auth/auth.module';
import { MarketDataModule } from './market-data/market-data.module';
import { SignalGenerationModule } from './signal-generation/signal-generation.module';
import { StrategyModule } from './strategy/strategy.module';
import { ExecutionModule } from './execution/execution.module';
import { BacktestModule } from './backtest/backtest.module';
import { OrchestratorModule } from './orchestrator/orchestrator.module';
import { PortfolioController } from './api/portfolio.controller';
import { SignalsController } from './api/signals.controller';
import { TradesController } from './api/trades.controller';
import { StrategyParamsController } from './api/strategy-params.controller';
import { BacktestController } from './api/backtest.controller';
import { HealthController, AuthController, UsageController } from './api/health.controller';
import { EventsGateway } from './events/events.gateway';
import { PortfolioService } from './api/portfolio.service';

@Module({
  imports: [
    ConfigModule.forRoot({ isGlobal: true }),
    ScheduleModule.forRoot(),
    BullModule.forRoot({
      connection: {
        host: process.env.REDIS_HOST || 'localhost',
        port: Number(process.env.REDIS_PORT || 6379),
      },
    }),
    DatabaseModule,
    RedisModule,
    AuthModule,
    MarketDataModule,
    SignalGenerationModule,
    StrategyModule,
    ExecutionModule,
    BacktestModule,
    OrchestratorModule,
  ],
  controllers: [
    HealthController,
    AuthController,
    PortfolioController,
    SignalsController,
    TradesController,
    StrategyParamsController,
    BacktestController,
    UsageController,
  ],
  providers: [EventsGateway, PortfolioService],
})
export class AppModule {}
