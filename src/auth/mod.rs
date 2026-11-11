use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AclRule {
    pub v: String, // "ALLOW" or "A", "DENY" or "D"
    pub p: String, // Regex or glob pattern
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    pub username: String,
    pub ver: i32,
    pub role: String,
    pub acl: Vec<AclRule>,
    pub exp: usize,
}

pub struct AuthManager {
    secret: Vec<u8>,
}

impl AuthManager {
    pub fn new(secret: &str) -> Self {
        Self {
            secret: secret.as_bytes().to_vec(),
        }
    }

    pub fn hash_password(password: &str) -> Result<String, String> {
        let salt = SaltString::generate(&mut OsRng);
        let argon2 = Argon2::default();
        let password_hash = argon2
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| e.to_string())?
            .to_string();
        Ok(password_hash)
    }

    pub fn verify_password(password: &str, password_hash: &str) -> bool {
        let parsed_hash = match PasswordHash::new(password_hash) {
            Ok(h) => h,
            Err(_) => return false,
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok()
    }

    pub fn create_jwt(
        &self,
        username: &str,
        role: &str,
        version: i32,
        rules: Vec<AclRule>,
    ) -> Result<String, jsonwebtoken::errors::Error> {
        let expiration = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as usize
            + 86400 * 7; // 7 days

        let claims = Claims {
            username: username.to_string(),
            ver: version,
            role: role.to_string(),
            acl: rules,
            exp: expiration,
        };

        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(&self.secret),
        )
    }

    pub fn verify_jwt(&self, token: &str) -> Result<Claims, jsonwebtoken::errors::Error> {
        let mut validation = Validation::default();
        validation.validate_exp = true;
        let token_data = decode::<Claims>(
            token,
            &DecodingKey::from_secret(&self.secret),
            &validation,
        )?;
        Ok(token_data.claims)
    }
}

pub struct AclEngine;

impl AclEngine {
    /// Check permission against user rules.
    /// Default is DENY all.
    /// Rules evaluated in order.
    pub fn check_permission(path: &str, rules: &[AclRule]) -> bool {
        let mut allowed = false;

        for rule in rules {
            if pattern_matches(&rule.p, path) {
                if rule.v.eq_ignore_ascii_case("ALLOW") || rule.v == "A" {
                    allowed = true;
                } else if rule.v.eq_ignore_ascii_case("DENY") || rule.v == "D" {
                    allowed = false;
                }
            }
        }

        allowed
    }
}

/// Matches path against a pattern which could be a regex or glob
pub fn pattern_matches(pattern: &str, path: &str) -> bool {
    // If it's already a regex (e.g. starts with ^ or contains .*)
    if pattern.starts_with('^') {
        if let Ok(re) = Regex::new(pattern) {
            return re.is_match(path);
        }
    }

    // Exact string match check
    if pattern == path {
        return true;
    }
    // Also consider trailing slashes equivalent for non-wildcard patterns (e.g. /_admin matches /_admin/)
    if !pattern.contains('*') && !pattern.contains('?') {
        let p_trimmed = pattern.trim_end_matches('/');
        let path_trimmed = path.trim_end_matches('/');
        if p_trimmed == path_trimmed {
            return true;
        }
    }

    // Convert glob to regex
    // e.g. /docs/* -> ^/docs/[^/]+$, /docs/** -> ^/docs/.*
    let mut regex_str = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    regex_str.push_str(".*");
                } else {
                    regex_str.push_str("[^/]*");
                }
            }
            '?' => regex_str.push_str("[^/]"),
            '.' | '+' | '(' | ')' | '[' | ']' | '{' | '}' | '^' | '$' | '|' | '\\' => {
                regex_str.push('\\');
                regex_str.push(c);
            }
            _ => regex_str.push(c),
        }
    }
    regex_str.push('$');

    if let Ok(re) = Regex::new(&regex_str) {
        if re.is_match(path) {
            return true;
        }
        // If path has trailing slash and didn't match, test path without trailing slash
        if path.ends_with('/') && path.len() > 1 && re.is_match(path.trim_end_matches('/')) {
            return true;
        }
    }
    false
}
