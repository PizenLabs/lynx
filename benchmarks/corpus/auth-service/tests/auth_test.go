package auth

import "testing"

// TestValidateTokenRejectsExpired is the behavioural specification for token
// validation: an expired token must be rejected.
func TestValidateTokenRejectsExpired(t *testing.T) {
	if ValidateToken("a.b.c") {
		t.Fatal("expected expired token to be rejected")
	}
}

// TestLoginRejectsEmptyCredentials pins the credential contract.
func TestLoginRejectsEmptyCredentials(t *testing.T) {
	svc := &AuthService{}
	if _, err := svc.Login("", ""); err == nil {
		t.Fatal("expected an error for empty credentials")
	}
}
