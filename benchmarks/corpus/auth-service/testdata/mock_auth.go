package auth

// TestLogin is a test double, not the real login path.
func TestLogin() bool { return true }

// MockValidateToken stands in for the real validator in tests only.
func MockValidateToken() bool { return true }
