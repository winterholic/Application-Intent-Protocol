//! MySQL 8.4 / MariaDB 11.4 login sessions with rotating refresh tokens (WBS 7.2.2).
//!
//! The original FaceReview reissues a refresh token without ever invalidating the one presented, so a stolen cookie keeps
//! minting access tokens until it expires. This store does not copy that defect: every refresh token is single use. A token
//! that was already used is a replay, which revokes its whole session (family) so the thief and the owner both have to log in
//! again. Only the SHA-256 of a token is stored. Every decision uses the database clock.
//!
//! The store never decides who a subject is: the service passes the subject it resolved from its own credential check, and
//! checks the subject is still allowed to log in between `consume` and `issue_next`.
use mysql_async::prelude::Queryable;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// What the service allows a session to live. A refresh token expires after `idle_secs` without use; the session never
/// outlives `absolute_secs`, however often it is refreshed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionPolicy {
    pub idle_secs: u32,
    pub absolute_secs: u32,
}

impl Default for SessionPolicy {
    /// 7 days between uses (the original cookie's `max_age`) and 30 days in total (the original token lifetime).
    fn default() -> Self {
        Self { idle_secs: 7 * 24 * 3600, absolute_secs: 30 * 24 * 3600 }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidSchema,
    InvalidSubject,
    InvalidPolicy,
    /// The session was revoked (logout, account suspension, replay) between `consume` and `issue_next`.
    Revoked,
    Database(String),
}

fn db(error: mysql_async::Error) -> Error {
    Error::Database(match &error {
        mysql_async::Error::Server(server) => format!("mysql errno {}", server.code),
        _ => "mysql connection error".to_string(),
    })
}

/// A refresh token the caller sets as a cookie. It is shown once; only its digest is stored.
#[derive(Debug, PartialEq, Eq)]
pub struct Issued {
    pub family: String,
    pub refresh_token: String,
    /// Unix seconds, from the database clock.
    pub expires_at: i64,
}

/// A refresh token that was valid and is now used up. The caller checks the subject and then calls `issue_next` or
/// `revoke_family`, in the same transaction.
#[derive(Debug, PartialEq, Eq)]
pub struct Consumed {
    pub family: String,
    pub subject: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Consume {
    Fresh(Consumed),
    /// The token was used before. The session is revoked now; the caller must commit that, not roll back.
    Replayed {
        family: String,
        subject: String,
    },
    Expired,
    Revoked,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct MysqlAuthSessions {
    schema: String,
    policy: SessionPolicy,
}

fn digest(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

fn new_token() -> String {
    // Two random UUIDs: 244 random bits, ASCII only.
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn valid_subject(subject: &str) -> bool {
    !subject.is_empty() && subject.len() <= 255 && !subject.chars().any(char::is_control)
}

impl MysqlAuthSessions {
    pub fn new(schema: impl Into<String>, policy: SessionPolicy) -> Result<Self, Error> {
        let schema = schema.into();
        let bytes = schema.as_bytes();
        if bytes.len() > 63
            || !bytes.starts_with(b"aip_")
            || !bytes.get(4).is_some_and(u8::is_ascii_lowercase)
            || !bytes[5..].iter().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'_')
        {
            return Err(Error::InvalidSchema);
        }
        if policy.idle_secs == 0 || policy.absolute_secs < policy.idle_secs {
            return Err(Error::InvalidPolicy);
        }
        Ok(Self { schema, policy })
    }

    fn sessions(&self) -> String {
        format!("`{}`.`aip_auth_sessions`", self.schema)
    }

    fn tokens(&self) -> String {
        format!("`{}`.`aip_auth_refresh_tokens`", self.schema)
    }

    /// Starts a session for a subject the service has just authenticated.
    pub async fn open<Q: Queryable>(&self, conn: &mut Q, subject: &str) -> Result<Issued, Error> {
        if !valid_subject(subject) {
            return Err(Error::InvalidSubject);
        }
        let family = Uuid::new_v4().to_string();
        conn.exec_drop(
            format!(
                "INSERT INTO {} (family, subject, created_at, absolute_expires_at) \
                 VALUES (?, ?, UTC_TIMESTAMP(6), DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND))",
                self.sessions()
            ),
            (&family, subject, self.policy.absolute_secs),
        )
        .await
        .map_err(db)?;
        self.insert_token(conn, &family).await
    }

    async fn insert_token<Q: Queryable>(&self, conn: &mut Q, family: &str) -> Result<Issued, Error> {
        let token = new_token();
        // The token lives for the idle window, but never past the session's absolute end, and not for a revoked session.
        conn.exec_drop(
            format!(
                "INSERT INTO {} (token_hash, family, state, created_at, expires_at) \
                 SELECT ?, family, 'ACTIVE', UTC_TIMESTAMP(6), LEAST(DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND), absolute_expires_at) \
                 FROM {} WHERE family = ? AND revoked_at IS NULL",
                self.tokens(),
                self.sessions()
            ),
            (digest(&token), self.policy.idle_secs, family),
        )
        .await
        .map_err(db)?;
        let inserted: Option<u64> = conn.query_first("SELECT ROW_COUNT()").await.map_err(db)?;
        if inserted != Some(1) {
            return Err(Error::Revoked);
        }
        let expires: Option<i64> = conn
            .exec_first(
                format!(
                    "SELECT CAST(TIMESTAMPDIFF(SECOND, '1970-01-01 00:00:00', expires_at) AS SIGNED) FROM {} WHERE token_hash = ?",
                    self.tokens()
                ),
                (digest(&token),),
            )
            .await
            .map_err(db)?;
        Ok(Issued { family: family.to_owned(), refresh_token: token, expires_at: expires.ok_or_else(|| Error::Database("token vanished".into()))? })
    }

    /// Uses up the presented refresh token. The row is locked, so two requests with the same token cannot both win: the second
    /// one sees a used token, which is a replay.
    pub async fn consume<Q: Queryable>(&self, conn: &mut Q, presented: &str) -> Result<Consume, Error> {
        if presented.is_empty() || presented.len() > 256 {
            return Ok(Consume::Unknown);
        }
        type Row = (String, String, String, bool, bool);
        let row: Option<Row> = conn
            .exec_first(
                format!(
                    "SELECT t.family, s.subject, t.state, s.revoked_at IS NOT NULL, \
                            t.expires_at <= UTC_TIMESTAMP(6) OR s.absolute_expires_at <= UTC_TIMESTAMP(6) \
                     FROM {} t JOIN {} s ON s.family = t.family WHERE t.token_hash = ? FOR UPDATE",
                    self.tokens(),
                    self.sessions()
                ),
                (digest(presented),),
            )
            .await
            .map_err(db)?;
        let Some((family, subject, state, revoked, expired)) = row else { return Ok(Consume::Unknown) };
        if revoked {
            return Ok(Consume::Revoked);
        }
        if state == "USED" {
            self.revoke_family(conn, &family, "replay").await?;
            return Ok(Consume::Replayed { family, subject });
        }
        if expired {
            return Ok(Consume::Expired);
        }
        conn.exec_drop(format!("UPDATE {} SET state = 'USED' WHERE token_hash = ? AND state = 'ACTIVE'", self.tokens()), (digest(presented),))
            .await
            .map_err(db)?;
        Ok(Consume::Fresh(Consumed { family, subject }))
    }

    /// The next refresh token of a session whose previous one was just consumed.
    pub async fn issue_next<Q: Queryable>(&self, conn: &mut Q, consumed: &Consumed) -> Result<Issued, Error> {
        self.insert_token(conn, &consumed.family).await
    }

    pub async fn revoke_family<Q: Queryable>(&self, conn: &mut Q, family: &str, reason: &str) -> Result<bool, Error> {
        conn.exec_drop(
            format!("UPDATE {} SET revoked_at = UTC_TIMESTAMP(6), revoked_reason = ? WHERE family = ? AND revoked_at IS NULL", self.sessions()),
            (reason, family),
        )
        .await
        .map_err(db)?;
        let changed: Option<u64> = conn.query_first("SELECT ROW_COUNT()").await.map_err(db)?;
        Ok(changed == Some(1))
    }

    /// Ends every session of a subject, for a suspended or withdrawn account. Returns how many sessions it ended.
    pub async fn revoke_subject<Q: Queryable>(&self, conn: &mut Q, subject: &str, reason: &str) -> Result<u64, Error> {
        conn.exec_drop(
            format!("UPDATE {} SET revoked_at = UTC_TIMESTAMP(6), revoked_reason = ? WHERE subject = ? AND revoked_at IS NULL", self.sessions()),
            (reason, subject),
        )
        .await
        .map_err(db)?;
        let changed: Option<u64> = conn.query_first("SELECT ROW_COUNT()").await.map_err(db)?;
        Ok(changed.unwrap_or(0))
    }

    /// Logout: ends the session that owns the presented token, whichever state the token is in. Returns whether a session ended.
    pub async fn revoke_by_token<Q: Queryable>(&self, conn: &mut Q, presented: &str, reason: &str) -> Result<bool, Error> {
        if presented.is_empty() || presented.len() > 256 {
            return Ok(false);
        }
        let family: Option<String> =
            conn.exec_first(format!("SELECT family FROM {} WHERE token_hash = ?", self.tokens()), (digest(presented),)).await.map_err(db)?;
        match family {
            Some(family) => self.revoke_family(conn, &family, reason).await,
            None => Ok(false),
        }
    }

    /// True while the session exists, is not revoked and has not reached its absolute end. Access tokens carry the family
    /// as their session id and are only honoured while this holds.
    pub async fn is_active<Q: Queryable>(&self, conn: &mut Q, family: &str, subject: &str) -> Result<bool, Error> {
        let row: Option<u8> = conn
            .exec_first(
                format!(
                    "SELECT 1 FROM {} WHERE family = ? AND subject = ? AND revoked_at IS NULL AND absolute_expires_at > UTC_TIMESTAMP(6)",
                    self.sessions()
                ),
                (family, subject),
            )
            .await
            .map_err(db)?;
        Ok(row.is_some())
    }

    /// Deletes sessions that ended (expired or revoked) at least `grace_secs` ago with their tokens, at most `limit` per call.
    /// A replayed token is only recognised while its row exists, so the grace period is how long a stolen token stays detectable.
    pub async fn purge<Q: Queryable>(&self, conn: &mut Q, grace_secs: u32, limit: u32) -> Result<u64, Error> {
        let doomed: Vec<String> = conn
            .exec(
                format!(
                    "SELECT family FROM {} WHERE LEAST(absolute_expires_at, COALESCE(revoked_at, absolute_expires_at)) \
                     <= DATE_SUB(UTC_TIMESTAMP(6), INTERVAL ? SECOND) LIMIT ?",
                    self.sessions()
                ),
                (grace_secs, limit),
            )
            .await
            .map_err(db)?;
        for family in &doomed {
            conn.exec_drop(format!("DELETE FROM {} WHERE family = ?", self.tokens()), (family,)).await.map_err(db)?;
            conn.exec_drop(format!("DELETE FROM {} WHERE family = ?", self.sessions()), (family,)).await.map_err(db)?;
        }
        Ok(doomed.len() as u64)
    }
}

/// AIP-owned additive infrastructure shared by bootstrap and adoption.
pub(crate) fn auth_session_ddl(meta_schema: &str) -> Vec<String> {
    vec![
        format!(
            "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_auth_sessions` (\
               family CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL PRIMARY KEY, \
               subject VARCHAR(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_bin NOT NULL, \
               created_at DATETIME(6) NOT NULL, absolute_expires_at DATETIME(6) NOT NULL, \
               revoked_at DATETIME(6) NULL, revoked_reason VARCHAR(32) CHARACTER SET ascii COLLATE ascii_bin NULL, \
               KEY aip_auth_sessions_subject (subject, revoked_at), KEY aip_auth_sessions_end (absolute_expires_at)) ENGINE=InnoDB"
        ),
        format!(
            "CREATE TABLE IF NOT EXISTS `{meta_schema}`.`aip_auth_refresh_tokens` (\
               token_hash BINARY(32) NOT NULL PRIMARY KEY, family CHAR(36) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, \
               state VARCHAR(8) CHARACTER SET ascii COLLATE ascii_bin NOT NULL, created_at DATETIME(6) NOT NULL, expires_at DATETIME(6) NOT NULL, \
               KEY aip_auth_refresh_family (family), CONSTRAINT aip_auth_refresh_state CHECK (state IN ('ACTIVE', 'USED'))) ENGINE=InnoDB"
        ),
    ]
}
