const BASE_URL_KEY = 'cp_base_url';
const TOKEN_KEY = 'cp_token';

export function getBaseUrl(): string {
  return localStorage.getItem(BASE_URL_KEY) ?? '';
}

export function getToken(): string | null {
  return localStorage.getItem(TOKEN_KEY);
}

export function setCredentials(baseUrl: string, token: string): void {
  localStorage.setItem(BASE_URL_KEY, baseUrl);
  localStorage.setItem(TOKEN_KEY, token);
  // Cookie for SSE same-origin auth: EventSource can't send an Authorization header,
  // so the backend reads `cp_token` from the cookie. Name MUST match the backend
  // (app/auth.py reads request.cookies.get("cp_token")).
  document.cookie = `cp_token=${token}; path=/; SameSite=Lax`;
}

export function clearCredentials(): void {
  localStorage.removeItem(BASE_URL_KEY);
  localStorage.removeItem(TOKEN_KEY);
  document.cookie = 'cp_token=; path=/; expires=Thu, 01 Jan 1970 00:00:00 GMT';
}

export function isAuthenticated(): boolean {
  return getToken() !== null;
}
