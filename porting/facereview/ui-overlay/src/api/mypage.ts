import { aipBaseUrl } from './index';
import { aipWrites, authTransport } from './aipClient';
import { createMypageApi } from './mypageAdapter';

const mypage = createMypageApi({ baseUrl: aipBaseUrl, fetchImpl: authTransport.fetch });

export const sendEmailVerification = mypage.sendEmailVerification;
export const verifyEmailCode = mypage.verifyEmailCode;
export const verifyPasswordCode = mypage.verifyPasswordCode;
export const changePassword = mypage.changePassword;

export const withdraw = async () => {
  const result = await aipWrites.withdraw();
  return { status: 200, data: result };
};
