// The AIP transport reads the access token from the auth store before every request, so there is no axios default header left to set.
// The functions stay because the login page, the store and the logout flow of the pinned admin UI still call them.
export const setAuthToken = (_token: string | null): void => {};

const HeaderToken = {
  set: setAuthToken,
};

export default HeaderToken;
