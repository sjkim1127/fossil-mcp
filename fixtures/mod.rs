/// A small Rust sample used in integration tests.
pub const RUST_SAMPLE: &str = r#"
pub struct TokenStore {
    tokens: Vec<String>,
}

impl TokenStore {
    pub fn new() -> Self {
        Self { tokens: Vec::new() }
    }

    pub fn refresh_token(&mut self, old_token: &str) -> String {
        self.revoke_token(old_token);
        let new_token = generate_token();
        self.tokens.push(new_token.clone());
        new_token
    }

    fn revoke_token(&mut self, token: &str) {
        self.tokens.retain(|t| t != token);
    }
}

pub fn generate_token() -> String {
    "tok_abc123".to_string()
}

pub fn validate_token(token: &str) -> bool {
    !token.is_empty()
}
"#;

/// A small Python sample used in integration tests.
pub const PYTHON_SAMPLE: &str = r#"
class AuthClient:
    def __init__(self, base_url: str):
        self.base_url = base_url
        self._token = None

    def refresh_token(self, old_token: str) -> str:
        self.revoke_token(old_token)
        new_token = self._generate_token()
        self._token = new_token
        return new_token

    def revoke_token(self, token: str) -> None:
        self._token = None

    def _generate_token(self) -> str:
        return "py_tok_xyz"

def validate_token(token: str) -> bool:
    return bool(token)
"#;

/// A small TypeScript sample used in integration tests.
pub const TS_SAMPLE: &str = r#"
interface TokenStore {
    refresh(token: string): string;
}

class OAuthClient implements TokenStore {
    private token: string | null = null;

    refresh(old: string): string {
        this.revoke(old);
        return this.generate();
    }

    revoke(token: string): void {
        this.token = null;
    }

    private generate(): string {
        return "ts_tok_abc";
    }
}

function validateToken(token: string): boolean {
    return token.length > 0;
}
"#;
