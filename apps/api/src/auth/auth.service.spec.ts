import * as bcrypt from 'bcrypt';
import { AuthService } from './auth.service';

describe('AuthService', () => {
  it('returns access_token when credentials are valid', async () => {
    const passwordHash = await bcrypt.hash('admin123', 10);
    const userModel = {
      findOne: jest.fn().mockReturnValue({
        exec: jest.fn().mockResolvedValue({
          _id: 'user-1',
          email: 'admin@ngx.local',
          password_hash: passwordHash,
        }),
      }),
    };
    const jwt = { sign: jest.fn().mockReturnValue('signed-token') };

    const service = new AuthService(userModel as never, jwt as never);
    const result = await service.login('admin@ngx.local', 'admin123');

    expect(result).toEqual({ access_token: 'signed-token' });
    expect(jwt.sign).toHaveBeenCalledWith({ sub: 'user-1', email: 'admin@ngx.local' });
  });

  it('returns null for invalid password', async () => {
    const passwordHash = await bcrypt.hash('admin123', 10);
    const userModel = {
      findOne: jest.fn().mockReturnValue({
        exec: jest.fn().mockResolvedValue({
          _id: 'user-1',
          email: 'admin@ngx.local',
          password_hash: passwordHash,
        }),
      }),
    };
    const jwt = { sign: jest.fn() };

    const service = new AuthService(userModel as never, jwt as never);
    const result = await service.login('admin@ngx.local', 'wrong-password');

    expect(result).toBeNull();
    expect(jwt.sign).not.toHaveBeenCalled();
  });
});
