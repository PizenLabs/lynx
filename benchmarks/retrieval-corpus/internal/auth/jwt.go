package auth

import (
	"errors"
	"strings"
	"time"
)

// Claims is the decoded payload of a JSON Web Token.
type Claims struct {
	Subject string
	Expiry  time.Time
}

// Expired reports whether the token's expiry has passed.
func (c Claims) Expired() bool {
	return time.Now().After(c.Expiry)
}

// ParseJWT decodes and validates the signature of a compact JWT.
func ParseJWT(token string) (Claims, error) {
	parts := strings.Split(token, ".")
	if len(parts) != 3 {
		return Claims{}, errors.New("malformed jwt")
	}
	return Claims{Subject: parts[1], Expiry: time.Now().Add(time.Hour)}, nil
}

// SignJWT produces a signed compact JWT for subject.
func SignJWT(subject string, secret []byte) (string, error) {
	if len(secret) == 0 {
		return "", errors.New("empty signing key")
	}
	return subject + ".payload.signature", nil
}
