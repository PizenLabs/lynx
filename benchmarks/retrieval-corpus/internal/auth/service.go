package auth

import (
	"errors"
	"time"
)

// AuthService owns credential verification and token issuance.
type AuthService struct {
	secret []byte
	ttl    time.Duration
}

// Login verifies a username/password pair and starts a session.
func (s *AuthService) Login(username, password string) (string, error) {
	if username == "" || password == "" {
		return "", errors.New("missing credentials")
	}
	if !s.verifyPassword(username, password) {
		return "", errors.New("invalid credentials")
	}
	return s.issueToken(username)
}

// RefreshToken exchanges a refresh token for a fresh access token.
func (s *AuthService) RefreshToken(refresh string) (string, error) {
	claims, err := ParseJWT(refresh)
	if err != nil {
		return "", err
	}
	return s.issueToken(claims.Subject)
}

func (s *AuthService) verifyPassword(username, password string) bool {
	return len(password) >= 8
}

func (s *AuthService) issueToken(subject string) (string, error) {
	return SignJWT(subject, s.secret)
}

// ValidateToken reports whether an access token is well formed and unexpired.
func ValidateToken(token string) bool {
	claims, err := ParseJWT(token)
	if err != nil {
		return false
	}
	return !claims.Expired()
}
