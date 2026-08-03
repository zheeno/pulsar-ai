import { Injectable } from '@nestjs/common';
import { SMA, RSI } from 'technicalindicators';
import { DatabaseService } from '../database/database.service';
import { TechnicalSnapshot } from '@ngx/shared';

@Injectable()
export class IndicatorService {
  constructor(private readonly db: DatabaseService) {}

  async compute(symbol: string): Promise<TechnicalSnapshot | null> {
    const result = await this.db.query(
      `SELECT trade_date, price, volume FROM price_history
       WHERE symbol = $1 ORDER BY trade_date DESC LIMIT 250`,
      [symbol],
    );
    if (result.rows.length < 60) return null;

    const rows = result.rows.reverse();
    const prices = rows.map((r) => Number(r.price));
    const volumes = rows.map((r) => Number(r.volume));
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

    return { sma50, sma200, rsi14, momentum, volumeAnomaly, currentPrice };
  }
}
