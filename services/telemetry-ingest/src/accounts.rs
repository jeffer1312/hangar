//! Contas do painel: e-mail e senha (argon2), sessões e convites de uso único.
//! Token de sessão e de convite sai daqui uma vez e só o sha256 dele fica no banco.
use std::sync::LazyLock;

use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

pub const SESSION_DAYS: i32 = 30;
pub const INVITE_DAYS: i32 = 7;
pub const MIN_PASSWORD: usize = 10;

pub fn hash_password(password: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default().hash_password(password.as_bytes(), &salt).expect("argon2 com parâmetros padrão").to_string()
}

fn verify(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
}

/// argon2 leva dezenas de milissegundos: fora do executor do tokio.
pub async fn hash_off_thread(password: String) -> String {
    tokio::task::spawn_blocking(move || hash_password(&password)).await.expect("hash da senha")
}

pub fn new_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex(&bytes)
}

pub fn digest(token: &str) -> String { hex(&Sha256::digest(token.as_bytes())) }

fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }

/// Minúsculo e com cara de e-mail; o resto (existir, receber) não é conferido.
pub fn normalize_email(email: &str) -> Option<String> {
    let email = email.trim().to_lowercase();
    let looks = email.split_once('@').is_some_and(|(user, domain)| !user.is_empty() && domain.contains('.') && !domain.starts_with('.'));
    (looks && email.len() <= 254 && !email.chars().any(char::is_whitespace)).then_some(email)
}

/// Conferida quando o e-mail não existe, para a resposta levar o mesmo tempo e não dizer quem tem conta.
static DECOY: LazyLock<String> = LazyLock::new(|| hash_password("decoy-que-nunca-casa"));

pub async fn login(pool: &PgPool, email: &str, password: &str) -> Result<Option<i64>, sqlx::Error> {
    let row: Option<(i64, String)> = sqlx::query_as("SELECT id, password_hash FROM panel_users WHERE email = $1")
        .bind(email)
        .fetch_optional(pool)
        .await?;
    let (id, hash) = match row {
        Some((id, hash)) => (Some(id), hash),
        None => (None, DECOY.clone()),
    };
    let password = password.to_owned();
    let ok = tokio::task::spawn_blocking(move || verify(&password, &hash)).await.unwrap_or(false);
    Ok(id.filter(|_| ok))
}

pub async fn create_session(pool: &PgPool, user_id: i64) -> Result<String, sqlx::Error> {
    let token = new_token();
    sqlx::query("INSERT INTO panel_sessions (token_hash, user_id, expires_at) VALUES ($1, $2, now() + make_interval(days => $3))")
        .bind(digest(&token))
        .bind(user_id)
        .bind(SESSION_DAYS)
        .execute(pool)
        .await?;
    Ok(token)
}

pub async fn session_user(pool: &PgPool, token: &str) -> Result<Option<(i64, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT u.id, u.email FROM panel_sessions s JOIN panel_users u ON u.id = s.user_id
         WHERE s.token_hash = $1 AND s.expires_at > now()",
    )
    .bind(digest(token))
    .fetch_optional(pool)
    .await
}

pub async fn end_session(pool: &PgPool, token: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM panel_sessions WHERE token_hash = $1").bind(digest(token)).execute(pool).await?;
    Ok(())
}

pub async fn create_invite(pool: &PgPool, by: i64) -> Result<String, sqlx::Error> {
    let token = new_token();
    sqlx::query("INSERT INTO panel_invites (token_hash, created_by, expires_at) VALUES ($1, $2, now() + make_interval(days => $3))")
        .bind(digest(&token))
        .bind(by)
        .bind(INVITE_DAYS)
        .execute(pool)
        .await?;
    Ok(token)
}

pub enum Join {
    Created(i64),
    BadInvite,
    EmailTaken,
}

/// Convite gasto e conta criada na mesma transação: dois pedidos com o mesmo convite não criam duas contas.
pub async fn join(pool: &PgPool, invite: &str, email: &str, password_hash: &str) -> Result<Join, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let open: Option<(String,)> = sqlx::query_as(
        "SELECT token_hash FROM panel_invites WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now() FOR UPDATE",
    )
    .bind(digest(invite))
    .fetch_optional(&mut *tx)
    .await?;
    let Some((token_hash,)) = open else { return Ok(Join::BadInvite) };
    let created: Option<(i64,)> =
        sqlx::query_as("INSERT INTO panel_users (email, password_hash) VALUES ($1, $2) ON CONFLICT (email) DO NOTHING RETURNING id")
            .bind(email)
            .bind(password_hash)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((id,)) = created else { return Ok(Join::EmailTaken) };
    sqlx::query("UPDATE panel_invites SET used_at = now(), used_by = $2 WHERE token_hash = $1")
        .bind(&token_hash)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Join::Created(id))
}

/// Conta criada ou senha trocada pela linha de comando do servidor (a primeira conta nasce assim).
pub async fn set_user(pool: &PgPool, email: &str, password_hash: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO panel_users (email, password_hash) VALUES ($1, $2)
         ON CONFLICT (email) DO UPDATE SET password_hash = EXCLUDED.password_hash",
    )
    .bind(email)
    .bind(password_hash)
    .execute(pool)
    .await?;
    // Senha nova derruba as sessões abertas com a antiga.
    sqlx::query("DELETE FROM panel_sessions WHERE user_id = (SELECT id FROM panel_users WHERE email = $1)")
        .bind(email)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn prune(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM panel_sessions WHERE expires_at < now()").execute(pool).await?;
    sqlx::query("DELETE FROM panel_invites WHERE expires_at < now() - interval '30 days'").execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_are_lowercased_and_need_a_domain() {
        assert_eq!(normalize_email("  Jeffer@Example.COM "), Some("jeffer@example.com".into()));
        for bad in ["", "sem-arroba", "@x.com", "a@semponto", "a@.com", "a b@x.com"] {
            assert_eq!(normalize_email(bad), None, "{bad}");
        }
    }

    #[test]
    fn password_hash_checks_only_the_same_password() {
        let hash = hash_password("uma senha longa");
        assert!(verify("uma senha longa", &hash));
        assert!(!verify("outra senha longa", &hash));
        assert!(!verify("uma senha longa", "não é hash"));
    }

    #[test]
    fn tokens_are_random_and_stored_as_sha256() {
        let (a, b) = (new_token(), new_token());
        assert_ne!(a, b);
        assert_eq!(a.len(), 64);
        assert_eq!(digest("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
