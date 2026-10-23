import { aipBaseUrl } from './index';
import { authTransport } from './aipClient';
import { createMypageApi } from './mypageAdapter';

const mypage = createMypageApi({ baseUrl: aipBaseUrl, fetchImpl: authTransport.fetch });

export const sendEmailVerification = mypage.sendEmailVerification;
export const verifyEmailCode = mypage.verifyEmailCode;
export const verifyPasswordCode = mypage.verifyPasswordCode;
export const changePassword = mypage.changePassword;

// Withdrawal stays blocked until the explicit confirmation flow is wired to User.withdraw.
export const withdraw = async (): Promise<never> => {
  throw Object.assign(new Error('route is not ported to AIP'), { code: 'PORT.NOT_PORTED' });
};
