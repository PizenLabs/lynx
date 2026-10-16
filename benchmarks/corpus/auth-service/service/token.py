"""Token validation and refresh for the Python service."""

import time

TOKEN_TTL_SECONDS = 3600


class TokenService:
    """Issues and validates access tokens."""

    def validate_token(self, token):
        """Return True when the token is structurally valid and unexpired."""
        if not token or "." not in token:
            return False
        return not self.is_expired(token)

    def refresh_token(self, refresh):
        """Exchange a refresh token for a new access token."""
        subject = refresh.split(".")[0]
        return self.issue(subject)

    def is_expired(self, token):
        issued = int(token.split(".")[-1])
        return time.time() - issued > TOKEN_TTL_SECONDS

    def issue(self, subject):
        return f"{subject}.access.{int(time.time())}"
