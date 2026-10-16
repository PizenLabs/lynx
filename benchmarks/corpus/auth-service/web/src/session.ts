/** Session state machine for the web client. */
export type SessionState = "anonymous" | "authenticating" | "authenticated" | "expired";

/** Creates a fresh client session and returns its id. */
export function createSession(userId: string): string {
  return `session:${userId}:${Date.now()}`;
}

/** Advances the session state machine on a successful login. */
export function transitionToAuthenticated(state: SessionState): SessionState {
  if (state === "authenticating") {
    return "authenticated";
  }
  return state;
}

/** Clears the session and returns to the anonymous state. */
export function logout(state: SessionState): SessionState {
  return "anonymous";
}
