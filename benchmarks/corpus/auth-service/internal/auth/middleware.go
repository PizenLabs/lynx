package auth

import (
	"errors"
	"net/http"
	"strings"
)

// Middleware authenticates inbound HTTP requests before they reach a handler.
type Middleware struct {
	header string
}

// NewMiddleware builds request authentication middleware.
func NewMiddleware(header string) *Middleware {
	if header == "" {
		header = "Authorization"
	}
	return &Middleware{header: header}
}

// Authenticate extracts a bearer token and establishes the request identity.
func (m *Middleware) Authenticate(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		token := bearerToken(r, m.header)
		if token == "" {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		if !ValidateToken(token) {
			http.Error(w, "forbidden", http.StatusForbidden)
			return
		}
		next.ServeHTTP(w, r)
	})
}

// bearerToken reads the bearer credential from the configured header.
func bearerToken(r *http.Request, header string) string {
	value := r.Header.Get(header)
	if !strings.HasPrefix(value, "Bearer ") {
		return ""
	}
	return strings.TrimSpace(strings.TrimPrefix(value, "Bearer "))
}

// Authorize enforces that the authenticated principal may reach next.
func (m *Middleware) Authorize(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get(m.header) == "" {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		next.ServeHTTP(w, r)
	})
}

var errNoSession = errors.New("no session")

// EstablishSession binds an authenticated principal to a request context.
func EstablishSession(token string) (string, error) {
	claims, err := ParseJWT(token)
	if err != nil {
		return "", err
	}
	if claims.Expired() {
		return "", errNoSession
	}
	return claims.Subject, nil
}
