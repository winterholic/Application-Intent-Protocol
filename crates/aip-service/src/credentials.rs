//! Settings of the service's own password login (`credentials` in the service configuration). The login itself is the
//! MySQL adapter's `mysql_auth`; the settings are plain data and parse on every build.
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialsConfig {
    /// The resource whose rows are accounts; it is the deployment's actor.
    pub resource: String,
    /// Physical column holding the password hash. It must not be mapped to any resource field, so no read can return it.
    pub secret_column: String,
    /// Fields of `resource`, by their names in the definition.
    pub identifier_field: String,
    pub name_field: String,
    pub role_field: String,
    /// The account is active while this field is false. A NULL counts as not active, as in the original `filter_by(is_deleted=False)`.
    pub active_field: String,
    /// Fields a new account starts false in (tutorial and verification flags).
    #[serde(default)]
    pub false_fields: Vec<String>,
    /// Fields set to the database clock on insert, for tables without defaults.
    #[serde(default)]
    pub time_fields: Vec<String>,
    /// The role of every new account. It is not a request field.
    pub default_role: String,
    /// Genres chosen at signup, stored as rows of another resource.
    #[serde(default)]
    pub favorites: Option<FavoritesConfig>,
    /// Definition operations (`operation read`) that run `password_auth.verify_password` and `password_auth.hash_password`.
    pub verify_operation: String,
    pub hash_operation: String,
    /// RSA private key (PKCS#1 or PKCS#8 PEM) and its key id. The matching public key must be in `auth.jwks`.
    pub signing_key_file: PathBuf,
    pub kid: String,
    #[serde(default = "default_access_ttl")]
    pub access_ttl_secs: u32,
    #[serde(default = "default_idle_days")]
    pub refresh_idle_days: u32,
    #[serde(default = "default_absolute_days")]
    pub refresh_absolute_days: u32,
    #[serde(default = "default_cookie_name")]
    pub cookie_name: String,
    #[serde(default = "default_cookie_path")]
    pub cookie_path: String,
    /// A browser only keeps a `SameSite=None` cookie when it is `Secure`. Turn this off only for a loopback test.
    #[serde(default = "default_true")]
    pub cookie_secure: bool,
    #[serde(default = "default_same_site")]
    pub cookie_same_site: String,
    #[serde(default = "default_password_min")]
    pub password_min_chars: usize,
    #[serde(default = "default_password_max")]
    pub password_max_chars: usize,
    #[serde(default = "default_name_min")]
    pub name_min_chars: usize,
    #[serde(default = "default_name_max")]
    pub name_max_chars: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FavoritesConfig {
    pub resource: String,
    /// The field that points at the account, and the field holding the value (an enum of the definition).
    pub user_field: String,
    pub value_field: String,
    #[serde(default)]
    pub time_fields: Vec<String>,
    #[serde(default = "default_one")]
    pub min: usize,
}

fn default_true() -> bool {
    true
}
fn default_access_ttl() -> u32 {
    600
}
fn default_idle_days() -> u32 {
    7
}
fn default_absolute_days() -> u32 {
    30
}
fn default_cookie_name() -> String {
    "refresh_token".into()
}
fn default_cookie_path() -> String {
    "/auth".into()
}
fn default_same_site() -> String {
    "None".into()
}
fn default_password_min() -> usize {
    8
}
fn default_password_max() -> usize {
    128
}
fn default_name_min() -> usize {
    2
}
fn default_name_max() -> usize {
    10
}
fn default_one() -> usize {
    1
}
