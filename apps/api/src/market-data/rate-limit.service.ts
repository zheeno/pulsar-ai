import { Injectable, Logger } from '@nestjs/common';
import { RedisService } from '../redis/redis.service';
import { DatabaseService } from '../database/database.service';
import { logStart } from '../common/log.util';

const DAILY_LIMIT = 100;
const MINUTE_LIMIT = 10;

@Injectable()
export class RateLimitService {
  private readonly logger = new Logger(RateLimitService.name);

  constructor(
    private readonly redis: RedisService,
    private readonly db: DatabaseService,
  ) {}

  private dailyKey(): string {
    const today = new Date().toISOString().split('T')[0];
    return `ngx_pulse:daily:${today}`;
  }

  private minuteKey(): string {
    const now = new Date();
    const minute = `${now.toISOString().slice(0, 16)}`;
    return `ngx_pulse:minute:${minute}`;
  }

  async canMakeRequest(): Promise<boolean> {
    const log = logStart(this.logger, 'canMakeRequest');
    const dailyCount = Number(await this.redis.get(this.dailyKey()) || 0);
    const minuteCount = Number(await this.redis.get(this.minuteKey()) || 0);
    const allowed = dailyCount < DAILY_LIMIT && minuteCount < MINUTE_LIMIT;
    log.done({ allowed, dailyCount, minuteCount });
    return allowed;
  }

  async recordRequest(endpoint: string, authMode: 'session' | 'api_key' | 'mock' = 'api_key'): Promise<void> {
    const log = logStart(this.logger, 'recordRequest', { endpoint, authMode });
    const dailyKey = this.dailyKey();
    const minuteKey = this.minuteKey();
    await this.redis.incr(dailyKey);
    await this.redis.expire(dailyKey, 86400);
    await this.redis.incr(minuteKey);
    await this.redis.expire(minuteKey, 120);
    await this.db.query(
      'INSERT INTO ngx_pulse_usage_log (endpoint) VALUES ($1)',
      [`${authMode}:${endpoint}`],
    );
    log.done();
  }

  async getUsageToday(authMode: 'session' | 'api_key' | 'mock' = 'api_key'): Promise<{
    daily: number;
    limit: number | null;
    remaining: number | null;
    authMode: string;
  }> {
    const log = logStart(this.logger, 'getUsageToday', { authMode });
    const daily = Number(await this.redis.get(this.dailyKey()) || 0);
    if (authMode === 'session') {
      const usage = { daily, limit: null, remaining: null, authMode };
      log.done(usage);
      return usage;
    }
    const usage = {
      daily,
      limit: DAILY_LIMIT,
      remaining: Math.max(0, DAILY_LIMIT - daily),
      authMode,
    };
    log.done(usage);
    return usage;
  }
}
