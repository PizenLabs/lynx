package auth

import "errors"

// AdminService authenticates operators against the admin realm.
type AdminService struct{}

// Login authenticates an administrator. The name is deliberately shared with
// the customer-facing auth::AuthService::Login to exercise ambiguity handling.
func (s *AdminService) Login(username, password string) (string, error) {
	if username != "root" {
		return "", errors.New("unknown admin")
	}
	return "admin-session", nil
}

// RevokeSession invalidates an administrative session.
func (s *AdminService) RevokeSession(id string) error {
	if id == "" {
		return errors.New("empty session id")
	}
	return nil
}
