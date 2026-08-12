import { Injectable, Logger } from '@nestjs/common';
import { InjectModel } from '@nestjs/mongoose';
import { Model } from 'mongoose';
import { JwtService } from '@nestjs/jwt';
import * as bcrypt from 'bcrypt';
import { User, UserDocument } from '../database/schemas';
import { logStart } from '../common/log.util';

@Injectable()
export class AuthService {
  private readonly logger = new Logger(AuthService.name);

  constructor(
    @InjectModel(User.name) private readonly userModel: Model<UserDocument>,
    private readonly jwt: JwtService,
  ) {}

  async login(email: string, password: string): Promise<{ access_token: string } | null> {
    const log = logStart(this.logger, 'login', { email });
    const user = await this.userModel.findOne({ email }).exec();
    if (!user) {
      log.warn('invalid credentials');
      log.done({ success: false });
      return null;
    }

    const valid = await bcrypt.compare(password, user.password_hash);
    if (!valid) {
      log.warn('invalid credentials');
      log.done({ success: false });
      return null;
    }

    const token = this.jwt.sign({ sub: user._id, email: user.email });
    log.done({ success: true, userId: user._id });
    return { access_token: token };
  }

  async validateUser(payload: { sub: string; email: string }) {
    const log = logStart(this.logger, 'validateUser', { userId: payload.sub });
    const user = { id: payload.sub, email: payload.email };
    log.done({ valid: true });
    return user;
  }
}
