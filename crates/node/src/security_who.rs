//! The security snapshot's view of a request's identity: a stored user by
//! name, or a token's user with the backend roles its token carried.

use oss_coordinator::security::Who;
use oss_rest::Identity;

/// What `identity` is to the security configuration.
pub fn who(identity: &Identity) -> Who<'_> {
    match identity.token_roles() {
        Some(roles) => Who::token(identity.name(), roles),
        None => Who::stored(identity.name()),
    }
}
