import { Injectable, Logger, UnauthorizedException } from '@nestjs/common';
import { PassportStrategy } from '@nestjs/passport';
import { ExtractJwt, Strategy } from 'passport-jwt';
import { AuthService } from './auth.service';
import { logStart } from '../common/log.util';

@Injectable()
export class JwtStrategy extends PassportStrategy(Strategy) {
  private readonly logger = new Logger(JwtStrategy.name);

  constructor(private authService: AuthService) {
    super({
      jwtFromRequest: ExtractJwt.fromAuthHeaderAsBearerToken(),
      ignoreExpiration: false,
      secretOrKey: process.env.SUPABASE_JWT_SECRET || 'dev-jwt-secret-change-in-production',
    });
  }

  async validate(payload: { sub: string; email: string }) {
    const log = logStart(this.logger, 'validate', { userId: payload.sub });
    const user = await this.authService.validateUser(payload);
    if (!user) {
      log.warn('unauthorized');
      throw new UnauthorizedException();
    }
    log.done({ userId: user.id });
    return user;
  }
}
