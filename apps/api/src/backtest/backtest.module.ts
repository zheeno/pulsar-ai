import { Module } from '@nestjs/common';
import { SignalGenerationModule } from '../signal-generation/signal-generation.module';
import { StrategyModule } from '../strategy/strategy.module';
import { ExecutionModule } from '../execution/execution.module';
import { BacktestService } from './backtest.service';

@Module({
  imports: [SignalGenerationModule, StrategyModule, ExecutionModule],
  providers: [BacktestService],
  exports: [BacktestService],
})
export class BacktestModule {}
