#!/usr/bin/env python3
"""Generate the grounded discovery-task suite for the Lynx validation harness.

Writes:

  benchmarks/tasks/discovery.json   task definitions (id, category, primitive, query)
  benchmarks/truth/discovery.json   ground truth keyed by task id

Ground truth is derived from the `auth-service` corpus checked into
`benchmarks/corpus/auth-service` and from a hand-verified reading of its source.
The suite is deterministic: re-running this script reproduces the same files.

Categories and counts (>= 100 tasks total):
  exact        35   bare symbol / fqdn resolution
  lexical      20   keyword-overlap natural language
  semantic     20   paraphrase (little lexical overlap)
  structural   15   exact coordinate lookup (resolve)
  relation     15   call-graph / containment edges
  context      10   compiled context packages
  ambiguity    10   shared names + noise
  adversarial  10   empty/hostile/oversized inputs
"""
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))
BENCH = os.path.dirname(HERE)

# ---------------------------------------------------------------- exact (35)
EXACT = [
    "ParseJWT", "SignJWT", "ValidateToken", "RefreshToken", "issueToken",
    "verifyPassword", "Authenticate", "Authorize", "bearerToken",
    "EstablishSession", "NewMiddleware", "Expired", "RevokeSession",
    "ExpirationSeconds", "DefaultTokenConfig", "validate_token",
    "refresh_token", "is_expired", "issue", "createSession",
    "transitionToAuthenticated", "logout", "rank", "Candidate", "Retriever",
    "TokenConfig", "AdminService", "Middleware", "TokenService", "Claims",
    "AuthService", "auth::ValidateToken",
    "service::token::TokenService.validate_token",
    "auth::AuthService::Login", "config::TokenConfig::ExpirationSeconds",
]
EXACT_ANSWER = {
    "ParseJWT": "auth::ParseJWT",
    "SignJWT": "auth::SignJWT",
    "ValidateToken": "auth::ValidateToken",
    "RefreshToken": "auth::AuthService::RefreshToken",
    "issueToken": "auth::AuthService::issueToken",
    "verifyPassword": "auth::AuthService::verifyPassword",
    "Authenticate": "auth::Middleware::Authenticate",
    "Authorize": "auth::Middleware::Authorize",
    "bearerToken": "auth::bearerToken",
    "EstablishSession": "auth::EstablishSession",
    "NewMiddleware": "auth::NewMiddleware",
    "Expired": "auth::Claims::Expired",
    "RevokeSession": "auth::AdminService::RevokeSession",
    "ExpirationSeconds": "config::TokenConfig::ExpirationSeconds",
    "DefaultTokenConfig": "config::DefaultTokenConfig",
    "validate_token": "service::token::TokenService.validate_token",
    "refresh_token": "service::token::TokenService.refresh_token",
    "is_expired": "service::token::TokenService.is_expired",
    "issue": "service::token::TokenService.issue",
    "createSession": "web::src::session::createSession",
    "transitionToAuthenticated": "web::src::session::transitionToAuthenticated",
    "logout": "web::src::session::logout",
    "rank": "retriever::Retriever::rank",
    "Candidate": "retriever::Candidate",
    "Retriever": "retriever::Retriever",
    "TokenConfig": "config::TokenConfig",
    "AdminService": "auth::AdminService",
    "Middleware": "auth::Middleware",
    "TokenService": "service::token::TokenService",
    "Claims": "auth::Claims",
    "AuthService": "auth::AuthService",
    "auth::ValidateToken": "auth::ValidateToken",
    "service::token::TokenService.validate_token":
        "service::token::TokenService.validate_token",
    "auth::AuthService::Login": "auth::AuthService::Login",
    "config::TokenConfig::ExpirationSeconds":
        "config::TokenConfig::ExpirationSeconds",
}

# -------------------------------------------------------------- lexical (20)
LEXICAL = [
    ("jwt parsing", ["auth::ParseJWT", "auth::SignJWT"]),
    ("token signature", ["auth::SignJWT"]),
    ("credential verification", ["auth::AuthService::verifyPassword"]),
    ("password check", ["auth::AuthService::verifyPassword"]),
    ("bearer header", ["auth::bearerToken", "auth::Middleware::Authenticate"]),
    ("session creation", ["web::src::session::createSession"]),
    ("session logout", ["web::src::session::logout"]),
    ("token expiry", ["auth::Claims::Expired",
                      "service::token::TokenService.is_expired"]),
    ("refresh token", ["auth::AuthService::RefreshToken",
                       "service::token::TokenService.refresh_token"]),
    ("access token issue", ["auth::AuthService::issueToken",
                            "service::token::TokenService.issue"]),
    ("reciprocal rank fusion", ["retriever::Retriever::rank"]),
    ("retriever candidates", ["retriever::Retriever", "retriever::Candidate"]),
    ("token ttl config", ["config::TokenConfig",
                          "config::DefaultTokenConfig",
                          "config::TokenConfig::ExpirationSeconds"]),
    ("revoke admin session", ["auth::AdminService::RevokeSession"]),
    ("authorize handler", ["auth::Middleware::Authorize"]),
    ("authenticate middleware", ["auth::Middleware::Authenticate"]),
    ("claims expiry", ["auth::Claims::Expired"]),
    ("candidate score", ["retriever::Candidate"]),
    ("establish session", ["auth::EstablishSession"]),
    ("middleware bearer token", ["auth::bearerToken", "auth::NewMiddleware"]),
]

SEMANTIC = [
    ("how are user credentials checked",
     ["auth::AuthService::Login", "auth::AuthService::verifyPassword"]),
    ("decide a token is no longer valid",
     ["auth::Claims::Expired", "service::token::TokenService.is_expired"]),
    ("create a signed credential for a subject",
     ["auth::SignJWT", "auth::AuthService::issueToken"]),
    ("exchange a refresh credential for a new access one",
     ["auth::AuthService::RefreshToken",
      "service::token::TokenService.refresh_token"]),
    ("read the incoming authorization header", ["auth::bearerToken"]),
    ("move an anonymous user into an authenticated state",
     ["web::src::session::transitionToAuthenticated"]),
    ("end the user's web session", ["web::src::session::logout"]),
    ("how long an access token remains usable",
     ["config::TokenConfig::ExpirationSeconds", "config::TokenConfig"]),
    ("guard an endpoint from unauthenticated callers",
     ["auth::Middleware::Authenticate", "auth::Middleware::Authorize"]),
    ("decode a compact jwt", ["auth::ParseJWT"]),
    ("bind an authenticated principal to the request",
     ["auth::EstablishSession"]),
    ("fuse two ranked result lists", ["retriever::Retriever::rank"]),
    ("start a fresh client session", ["web::src::session::createSession"]),
    ("administrator logs in", ["auth::AdminService::Login"]),
    ("invalidate an administrative session",
     ["auth::AdminService::RevokeSession"]),
    ("production token policy defaults", ["config::DefaultTokenConfig"]),
    ("assert an access token is well formed and unexpired",
     ["auth::ValidateToken"]),
    ("construct the request authentication middleware",
     ["auth::NewMiddleware"]),
    ("mint an access token for a subject",
     ["auth::AuthService::issueToken", "service::token::TokenService.issue"]),
    ("check that a token has not passed its expiry",
     ["auth::ValidateToken", "auth::Claims::Expired"]),
]

# ------------------------------------------------------------ structural (15)
STRUCTURAL = [
    "auth::Middleware::Authenticate", "auth::AuthService::RefreshToken",
    "service::token::TokenService", "retriever::Retriever::rank",
    "config::TokenConfig::ExpirationSeconds", "auth::AdminService",
    "web::src::session", "internal::auth::middleware", "auth::Claims::Expired",
    "auth::ValidateToken", "auth::EstablishSession",
    "service::token::TokenService.validate_token", "retriever::Candidate",
    "auth::Middleware", "config::DefaultTokenConfig",
]

# -------------------------------------------------------------- relation (15)
# (query, kind, expected endpoint fqdns)
RELATION = [
    ("auth::AuthService::Login", "Calls",
     ["auth::AuthService::issueToken", "auth::AuthService::verifyPassword"]),
    ("auth::AuthService::RefreshToken", "Calls",
     ["auth::AuthService::issueToken"]),
    ("auth::Middleware::Authenticate", "Calls", ["auth::bearerToken"]),
    ("service::token::TokenService.validate_token", "Calls",
     ["service::token::TokenService.is_expired"]),
    ("service::token::TokenService.refresh_token", "Calls",
     ["service::token::TokenService.issue"]),
    ("retriever::Retriever::rank", "Calls", ["retriever::Retriever::new"]),
    ("auth::AuthService::issueToken", "Calls",
     ["auth::AuthService::Login", "auth::AuthService::RefreshToken"]),
    ("auth::AuthService::verifyPassword", "Calls", ["auth::AuthService::Login"]),
    ("auth::bearerToken", "Calls", ["auth::Middleware::Authenticate"]),
    ("service::token::TokenService.is_expired", "Calls",
     ["service::token::TokenService.validate_token"]),
    ("service::token::TokenService.issue", "Calls",
     ["service::token::TokenService.refresh_token"]),
    ("retriever::Retriever::new", "Calls", ["retriever::Retriever::rank"]),
    ("auth::AuthService", "Contains",
     ["auth::AuthService::Login", "auth::AuthService::RefreshToken",
      "auth::AuthService::issueToken", "auth::AuthService::verifyPassword"]),
    ("auth::Middleware", "Contains",
     ["auth::Middleware::Authenticate", "auth::Middleware::Authorize"]),
    ("retriever::Retriever", "Contains",
     ["retriever::Retriever::new", "retriever::Retriever::rank"]),
]

# --------------------------------------------------------------- context (10)
CONTEXT = [
    ("authentication flow", ["auth::AuthService::Login",
                             "auth::Middleware::Authenticate"]),
    ("token validation", ["auth::ValidateToken",
                          "service::token::TokenService.validate_token"]),
    ("token refresh", ["auth::AuthService::RefreshToken",
                       "service::token::TokenService.refresh_token"]),
    ("request authorization", ["auth::Middleware::Authorize"]),
    ("session lifecycle", ["web::src::session::transitionToAuthenticated",
                           "web::src::session::logout"]),
    ("jwt parsing", ["auth::ParseJWT"]),
    ("token configuration", ["config::TokenConfig"]),
    ("retrieval ranking", ["retriever::Retriever::rank"]),
    ("credential verification", ["auth::AuthService::verifyPassword"]),
    ("access token issuance", ["auth::AuthService::issueToken"]),
]

# ------------------------------------------------------------- ambiguity (10)
AMBIGUITY = [
    ("Login", ["auth::AdminService::Login", "auth::AuthService::Login",
               "web::src::login::Login"]),
    ("TokenService", ["service::token::TokenService"]),
    ("Middleware", ["auth::Middleware"]),
    ("Claims", ["auth::Claims"]),
    ("Retriever", ["retriever::Retriever"]),
    ("Retriever::new", ["retriever::Retriever::new"]),
    ("TestLogin", ["auth::TestLogin"]),
    ("MockValidateToken", ["auth::MockValidateToken"]),
    ("Login", ["auth::AuthService::Login"]),
    ("Login", ["web::src::login::Login"]),
]

ADVERSARIAL = [
    ("", "no_error"),
    ("!!!not a symbol!!!", "results"),
    ("'; DROP TABLE symbols;--", "results"),
    ("a" * 5000, "no_error"),
    ("\u30e9\u30f3\u30c0\u30e0\u306a\u30af\u30a8\u30ea", "no_error"),
    ("token " * 2000, "no_error"),
    ("zzzz_nonexistent_symbol_zzzz", "results"),
    ("auth::AuthService::Login", "results"),
    ("how does authentication work?", "no_error"),
    ("\x00\x01\x02", "no_error"),
]


def build():
    tasks = []
    truth = {}

    def add(tid, category, primitive, query, primary, expected, **extra):
        task = {"id": tid, "category": category, "primitive": primitive,
                "query": query}
        task.update(extra)
        tasks.append(task)
        entry = {"primary": primary, "expected": expected}
        truth[tid] = entry

    for i, q in enumerate(EXACT, 1):
        ans = EXACT_ANSWER[q]
        add(f"exact-{i:02d}", "exact", "resolve", q, ans, [ans])

    for i, (q, exp) in enumerate(LEXICAL, 1):
        add(f"lexical-{i:02d}", "lexical", "search", q, exp[0], exp)

    for i, (q, exp) in enumerate(SEMANTIC, 1):
        add(f"semantic-{i:02d}", "semantic", "search", q, exp[0], exp)

    for i, q in enumerate(STRUCTURAL, 1):
        add(f"structural-{i:02d}", "structural", "resolve", q, q, [q])

    for i, (q, kind, exp) in enumerate(RELATION, 1):
        add(f"relation-{i:02d}", "relation", "relations", q, exp[0], exp,
            kind=kind)

    for i, (q, exp) in enumerate(CONTEXT, 1):
        add(f"context-{i:02d}", "context", "context", q, exp[0], exp)

    for i, (q, exp) in enumerate(AMBIGUITY, 1):
        primitive = "resolve_all"
        add(f"ambiguity-{i:02d}", "ambiguity", primitive, q, exp[0], exp)

    for i, (q, expectation) in enumerate(ADVERSARIAL, 1):
        add(f"adversarial-{i:02d}", "adversarial", "search", q, "", [],
            expect=expectation)

    return {"corpus": "auth-service", "tasks": tasks}, {"truth": truth}


def main():
    tasks_doc, truth_doc = build()
    os.makedirs(os.path.join(BENCH, "tasks"), exist_ok=True)
    os.makedirs(os.path.join(BENCH, "truth"), exist_ok=True)
    with open(os.path.join(BENCH, "tasks", "discovery.json"), "w") as fh:
        json.dump(tasks_doc, fh, indent=2, sort_keys=False)
        fh.write("\n")
    with open(os.path.join(BENCH, "truth", "discovery.json"), "w") as fh:
        json.dump(truth_doc, fh, indent=2, sort_keys=False)
        fh.write("\n")
    n = len(tasks_doc["tasks"])
    print(f"wrote {n} tasks across "
          f"{len(set(t['category'] for t in tasks_doc['tasks']))} categories")


if __name__ == "__main__":
    main()
