import { Module } from '@nestjs/common';
import { PositionSizingService, RiskPolicyService } from './risk-policy.service';
import { StrategyParamsService } from './strategy-params.service';

@Module({
  providers: [PositionSizingService, RiskPolicyService, StrategyParamsService],
  exports: [PositionSizingService, RiskPolicyService, StrategyParamsService],
})
export class StrategyModule {}
