/** Client-side mirrors of the server's rules in nn-auth, for instant feedback. */

export const MIN_USERNAME = 3;
export const MAX_USERNAME = 32;
export const MIN_PASSWORD = 10;

export function usernameError(username: string): string | undefined {
  const value = username.trim();
  if (value.length < MIN_USERNAME || value.length > MAX_USERNAME) {
    return `Use ${MIN_USERNAME}–${MAX_USERNAME} characters.`;
  }
  if (!/^[A-Za-z0-9_.-]+$/.test(value)) {
    return "Use only letters, numbers, '_', '.', and '-'.";
  }
  return undefined;
}

export function passwordError(password: string): string | undefined {
  if ([...password].length < MIN_PASSWORD) return `Use at least ${MIN_PASSWORD} characters.`;
  if (new TextEncoder().encode(password).length > 1024) return "Use at most 1024 bytes.";
  return undefined;
}
