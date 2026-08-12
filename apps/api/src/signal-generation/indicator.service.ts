import { Injectable, Logger } from '@nestjs/common';
import { InjectModel } from '@nestjs/mongoose';
import { Model } from 'mongoose';
import { SMA, RSI } from 'technicalindicators';
import { PriceHistory, PriceHistoryDocument } from '../database/schemas';
import { TechnicalSnapshot } from '@ngx/shared';
import { logStart } from '../common/log.util';

@Injectable()
export class IndicatorService {
  private readonly logger = new Logger(IndicatorService.name);

  constructor(
    @InjectModel(PriceHistory.name) private readonly priceModel: Model<PriceHistoryDocument>,
  ) {}

  async compute(symbol: string): Promise<TechnicalSnapshot | null> {
    const log = logStart(this.logger, 'compute', { symbol });
    const rows = await this.priceModel
      .find({ symbol })
      .sort({ trade_date: -1 })
      .limit(250)
      .exec();
    if (rows.length < 60) {
      log.debug('insufficient data', { rows: rows.length });
      log.done({ computed: false });
      return null;
    }

    const ordered = [...rows].reverse();
    const prices = ordered.map((r) => Number(r.price));
    const volumes = ordered.map((r) => Number(r.volume));
    const currentPrice = prices[prices.length - 1];

    const sma50Arr = SMA.calculate({ period: 50, values: prices });
    const sma200Arr = SMA.calculate({ period: 200, values: prices });
    const rsiArr = RSI.calculate({ period: 14, values: prices });

    const sma50 = sma50Arr.length ? sma50Arr[sma50Arr.length - 1] : null;
    const sma200 = sma200Arr.length ? sma200Arr[sma200Arr.length - 1] : null;
    const rsi14 = rsiArr.length ? rsiArr[rsiArr.length - 1] : null;

    const momentumWindow = 20;
    const momentum = prices.length > momentumWindow
      ? ((currentPrice - prices[prices.length - momentumWindow - 1]) / prices[prices.length - momentumWindow - 1]) * 100
      : null;

    const recentVolumes = volumes.slice(-20);
    const avgVolume = recentVolumes.reduce((a, b) => a + b, 0) / recentVolumes.length;
    const currentVolume = volumes[volumes.length - 1];
    const volumeAnomaly = avgVolume > 0 ? currentVolume / avgVolume : null;

    log.done({ currentPrice, rsi14, momentum });
    return { sma50, sma200, rsi14, momentum, volumeAnomaly, currentPrice };
  }
}
