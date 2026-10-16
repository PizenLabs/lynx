package config

import "time"

// TokenConfig holds the policy that governs access-token lifetime.
type TokenConfig struct {
	Issuer     string
	TTL        time.Duration
	RefreshTTL time.Duration
}

// DefaultTokenConfig returns the production token policy.
func DefaultTokenConfig() TokenConfig {
	return TokenConfig{
		Issuer:     "lynx-bench",
		TTL:        15 * time.Minute,
		RefreshTTL: 720 * time.Hour,
	}
}

// ExpirationSeconds is the access-token lifetime in seconds.
func (c TokenConfig) ExpirationSeconds() int {
	return int(c.TTL.Seconds())
}
